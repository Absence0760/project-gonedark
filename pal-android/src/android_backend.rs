//! Android PAL backend proper — `cargo-ndk` + `android-activity` + JNI shim (the ship target,
//! platforms.md §8). This module is `#[cfg(target_os = "android")]` (gated by its `mod` decl in
//! `lib.rs`), so it compiles to nothing on a desktop/CI host and never drags the Android deps
//! into the host build. It is the structural realization of build-order step 6: the
//! `android-activity` 0.6 entry point, the lifecycle/window/input pump, the wgpu(Vulkan) surface
//! bridge, and impls of the `gonedark_pal` traits backed by Android. (The pure thermal/battery
//! mapping seam lives in the host-compiled [`crate::thermal`] module so it can be unit-tested off
//! device; only its JNI reader is android-gated alongside this module.)
//!
//! # NOT device-verified
//! This was written against the *pinned* `android-activity` 0.6 / `jni` 0.22 / `ndk` 0.9 /
//! `wgpu` 29 / `oboe` 0.6 APIs. The **for-target build is verified on this workstation**
//! (NDK 28.2 + `cargo-ndk`): `cargo ndk -t arm64-v8a build -p gonedark-pal-android` passes in
//! dev and release. What remains **OWED is on-device shakeout** — actual audible/low-latency
//! AAudio output, surface/lifecycle behavior, input feel (see ../android/README.md). Spots that
//! are deferred or that need an
//! API sanity-check on real toolchain are flagged with `TODO(...)` / `NOTE:` inline.
//!
//! # Where `android_main` lives
//! The exported entry point (`android_main`) lives **here**, not in `app`. `android-activity`
//! generates the actual JNI `ANativeActivity_onCreate` glue from the `#[no_mangle]`
//! `android_main` symbol via its `native-activity` feature. `app` (whose desktop `fn main`
//! owns the run loop on the host) must, for the Android target, depend on this crate and
//! re-export / route to this entry — see the integrator note at the bottom of this file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gonedark_core::campaign::{Difficulty, NodeId};
use gonedark_core::components::{Army, Faction};
use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Optic};
use gonedark_engine::objectives::MissionStatus;
use gonedark_engine::{pixel_to_ndc, Game, OverlayClick, Scene, DEFAULT_SEED};
use gonedark_pal::mix::{oneshot_sound, scaled_gain, synth_bank, voice_from_cue, Mixer};
use gonedark_pal::{Audio, Input, InputFrame, SoundId, Storage, TouchSample, Window, MAX_TOUCHES};

// Bring oboe's stream-control traits into scope for the methods used below (`get_sample_rate`
// lives on `AudioStreamBase`, `request_start` on `AudioStream`).
use oboe::{AudioStream, AudioStreamBase};

use android_activity::input::{InputEvent, KeyAction, KeyEvent, Keycode, MotionAction, MotionEvent};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use log::{info, warn};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

// ---------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------

/// The native entry point. `android-activity`'s `native-activity` feature turns this
/// `#[no_mangle]` symbol into the JNI glue the `NativeActivity` calls on startup, handing
/// us the [`AndroidApp`] handle that owns the event loop, native window, and asset manager.
///
/// Loop shape (the canonical android-activity 0.6 pattern):
///   `loop { app.poll_events(timeout, |event| { ... }) }`
/// We block indefinitely between frames when idle and pump a frame on `MainEvent::RedrawNeeded`
/// (or, here, opportunistically while the window is up). The surface is created on
/// `InitWindow` and dropped on `TerminateWindow`, matching Android's surface lifecycle.
// android-activity's `native-activity` glue declares `android_main` with the Rust ABI
// (`extern "Rust"`) and calls it from `ANativeActivity_onCreate`; a plain `#[no_mangle] fn`
// matches that — `extern "C"` would mismatch the ABI and isn't FFI-safe for `AndroidApp`.
#[no_mangle]
fn android_main(app: AndroidApp) {
    // Route `log::*` into logcat. `adb logcat` / the dev loop (roadmap.md) reads this.
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("gonedark"),
    );
    // Without a hook, a Rust panic (and wgpu treats its errors as fatal = panic) prints to
    // stderr, which Android does NOT route to logcat — so the real cause is invisible and the
    // activity just dies. Route panics to logcat so InitWindow/GPU failures are diagnosable.
    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
    }));
    info!("android_main: starting Going Dark (Android PAL, Phase 1 scaffold)");

    // Build the PAL pieces. Window/Input wrap the AndroidApp; the RHI + the shared
    // `engine::Game` are created lazily on InitWindow (we have no GPU device until then).
    let mut window = AndroidWindow::new(app.clone());
    let mut input = AndroidInput::new(app.clone());
    let mut rhi: Option<AndroidRhi> = None;
    let mut game: Option<Game> = None;
    let mut last_frame = Instant::now();
    // Campaign win-reporting (Compose shell parity — the split-activity twin of the desktop host's
    // record-on-win, `app/src/main.rs`). `campaign_launch` is `Some((node, tier))` when this launch
    // is a campaign mission — the campaign `NodeId` index it maps to and the replay `diff` tier the
    // clear will be recorded at. On the first frame the match reads `Won`, `campaign_result_code`
    // latches the packed Activity result code the finish path hands back to the Compose shell (which
    // owns campaign progress in `SharedPreferences`). A non-campaign launch or a non-win never sets
    // it, so nothing is recorded — mirroring the desktop's "a loss records nothing".
    let mut campaign_launch: Option<(u32, u8)> = None;
    let mut campaign_result_code: Option<i32> = None;
    // Set when the post-match summary's REMATCH is tapped; drained after the overlay-click block
    // (which borrows `game`) to re-seed a fresh match of the same scene/loadout in place.
    let mut pending_rematch = false;

    // The Compose shell's launch payload (scene/loadout/prefs), read once off the launching
    // `Intent` (Compose shell parity, Tier 0). Read here at startup — the engine `Game` is built
    // lazily on `InitWindow`, possibly after a surface loss/recreate, and reuses this captured
    // config so every (re)build inside this process is consistent. Absent/malformed → a default
    // config (Scene::Skirmish, the real match — desktop's default boot), never a crash.
    let launch = read_launch_config(&app);
    info!(
        "launch config: scene={:?} loadout=({},{},{}) vol={}% sfx={}% sens={} invy={} diff={} node={} army={} cvd={} snd={}",
        launch.scene,
        launch.optic,
        launch.barrel,
        launch.magazine,
        launch.master_pct,
        launch.sfx_pct,
        launch.sens_x100,
        launch.invert_y,
        launch.diff,
        launch.node,
        launch.army,
        launch.colorblind_cues,
        launch.visual_sound_cues,
    );

    // On-device frame-rate + sim-checksum heartbeat (Phase 1 exit criterion: "running at
    // target frame rate on a target phone"). Throttled to ~one logcat line per second so it
    // doubles as a determinism eyeball without spamming. Read-only observation of `game` — it
    // never touches the loop, the sim, or the wall-clock `dt` driving `game.frame(...)`.
    let mut frame_count: u64 = 0; // total frames presented since process start
    let mut frames_since_report: u32 = 0; // frames presented since the last heartbeat line
    let mut last_report = Instant::now();

    let storage = AndroidStorage::new(app.clone());
    // Open the AAudio sink now so the stream is live before the first frame. Failure degrades to a
    // silent no-op (invariant #8) — never fatal.
    let mut audio = AndroidAudio::new();

    // Sanity-touch the stub PAL services so the deferred impls are linked, not dead code.
    let _ = storage.read("settings");
    audio.play_oneshot(0);

    // Apply the launch config's audio/look prefs at the PAL boundary (Compose shell parity, Tier 0
    // §5) — host/presentation only, never the sim (invariants #1/#2), exactly as the desktop shell
    // pushes them into `pal-desktop` outside the deterministic input frame. Done once here (the prefs
    // are match-setup, not per-tick); the audio scaling then rides every queued voice via the shared
    // `gonedark_pal::mix::scaled_gain`, mirroring `pal-desktop::audio::set_gains`. The pure
    // percent→gain / ×100→multiplier decode lives in the host-tested `crate::launch` mappers.
    audio.set_gains(
        crate::launch::pct_to_gain(launch.master_pct),
        crate::launch::pct_to_gain(launch.sfx_pct),
    );
    input.set_look_prefs(
        crate::launch::sens_x100_to_f32(launch.sens_x100),
        launch.invert_y,
    );

    // Real WS-C thermal/battery reader (Phase 4): the live render-tuning signal. Built once here and
    // passed into every `game.frame(...)` below, so the engine's `RenderTuning` controller reacts to
    // on-device heat (FPS cap + dyn-res floor) — these are the numbers that may reopen the D21
    // dual-rate question (phase-4-plan §C). It is a render-tuning *signal*, NEVER a sim input
    // (invariant #1/#4): the engine reads it through the PAL `ThermalSensor` trait (invariant #2) and
    // the per-tick checksum is unaffected. The decision logic is the host-tested pure mappers in
    // `crate::thermal`; this is only the (un-testable) JNI fetch. Poll once now so the first reading
    // lands in logcat and the JNI path is linked before frame one.
    let thermal = crate::thermal::AndroidThermalSensor::new(app.clone());
    {
        use gonedark_pal::ThermalSensor;
        info!(
            "thermal: initial state {:?} | power {:?}",
            thermal.thermal_state(),
            thermal.power_state(),
        );
    }

    // Android drives the SAME platform-agnostic loop the desktop host does: `engine::Game`
    // owns the deterministic sim + renderer + fixed-tick + cameras. Here we only own the
    // Android surface/input/lifecycle and feed `Game::frame` an InputFrame + a wall-clock dt
    // + the acquired surface view each iteration (mirroring app/src/main.rs).

    'outer: loop {
        // Block forever when idle; android-activity wakes us on the next event. A real
        // frame loop would use a short/zero timeout while the window is up so it can
        // render continuously — kept conservative here.
        let timeout = if window.surface_up {
            Some(std::time::Duration::ZERO)
        } else {
            None
        };

        app.poll_events(timeout, |event| {
            // android-activity also surfaces non-Main events (config changes, etc.); we only
            // act on Main lifecycle events here.
            let PollEvent::Main(main_event) = event else {
                return;
            };
            match main_event {
                MainEvent::InitWindow { .. } => {
                    info!("MainEvent::InitWindow — creating wgpu surface");
                    window.surface_up = true;
                    // The native window is only valid between InitWindow and TerminateWindow.
                    match app.native_window() {
                        Some(native_window) => match AndroidRhi::new(&app, native_window) {
                            Ok(new_rhi) => {
                                let (w, h) = window.size();
                                window.width = w;
                                window.height = h;
                                // Build the shared game against the live device, into the scene the
                                // Compose shell asked for (Tier 0). Same seed as desktop → the
                                // bit-identical deterministic scene. An unknown scene token falls
                                // back to the real playable match (Skirmish), matching the desktop
                                // host's default boot. The wire's gunsmith indices (opt/bar/mag,
                                // already clamped 0..=2 by the parser) map straight into the slot
                                // enums' `ALL` order and field the player's chosen build at match
                                // start via `new_scene_with_loadout` (WS-C / D60). For Skirmish the
                                // loadout is inert (no player loadout in that scene); for the
                                // campaign's `mission1` it applies to the player's troops as
                                // deterministic match-setup input — closing the gunsmith→mission loop.
                                // `Loadout::STANDARD` (all-zero indices) reproduces the pre-parity
                                // boot byte-for-byte.
                                // Build the shared game from the live device + the Compose launch
                                // config via the one shared builder (also used by the post-match
                                // REMATCH, so a rematch can't drift from the first boot). `Some(node)`
                                // marks a campaign launch for clear-recording; a fresh launch clears any
                                // stale win result.
                                let (new_game, cl) = build_match_game(&new_rhi, &launch);
                                campaign_launch = cl;
                                if cl.is_some() {
                                    campaign_result_code = None;
                                }
                                game = Some(new_game);
                                rhi = Some(new_rhi);
                                last_frame = Instant::now();
                                info!("wgpu surface + engine created at {w}x{h}");
                            }
                            Err(e) => warn!("RHI init failed: {e}"),
                        },
                        None => warn!("InitWindow with no native_window — skipping surface"),
                    }
                }
                MainEvent::TerminateWindow { .. } => {
                    info!("MainEvent::TerminateWindow — dropping surface + engine");
                    window.surface_up = false;
                    // Drop the game BEFORE the RHI: its renderer holds GPU resources owned by
                    // the device inside `rhi`, and the ANativeWindow is about to go invalid.
                    game = None;
                    rhi = None;
                }
                MainEvent::WindowResized { .. } => {
                    let (w, h) = window.size();
                    window.width = w;
                    window.height = h;
                    if let Some(rhi) = rhi.as_mut() {
                        rhi.resize(w, h);
                    }
                    info!("WindowResized -> {w}x{h}");
                }
                MainEvent::RedrawNeeded { .. } => {
                    // Rendering happens once per loop iteration (below) while the surface is
                    // up, so RedrawNeeded needs no special handling — the continuous loop
                    // already keeps the frame fresh.
                }
                MainEvent::Resume { .. } => info!("MainEvent::Resume"),
                MainEvent::Pause => info!("MainEvent::Pause"),
                MainEvent::Stop => info!("MainEvent::Stop"),
                MainEvent::Start => info!("MainEvent::Start"),
                MainEvent::Destroy => {
                    info!("MainEvent::Destroy — exiting android_main");
                    window.destroy_requested = true;
                }
                MainEvent::SaveState { .. } => {
                    // TODO(phase2): persist a resume snapshot across process death. BLOCKED on two
                    // landings, deliberately NOT done here (one-commit-one-workstream): (1)
                    // `AndroidStorage::{read,write}` must be real — they are still stubs below; and
                    // (2) the sim serialize/restore path (D28's `core::persist` — the snapshot
                    // format is decided, code pending). Once both exist: serialize the `Game`'s sim
                    // state via D28's `Sim::serialize` and write the bytes through `AndroidStorage`,
                    // restoring on the next `InitWindow`. Gating on Storage first.
                }
                MainEvent::LowMemory => warn!("MainEvent::LowMemory"),
                _ => {}
            }
        });

        // Drain native input (touch → pointer for the command-layer tap) into one
        // engine-neutral frame, then drive the shared game loop: compute the wall-clock dt,
        // acquire a surface frame, and let `engine::Game` step the deterministic sim + render
        // the interpolated snapshot — exactly as the desktop host does in app/src/main.rs.
        let mut input_frame: InputFrame = input.poll();

        // Back-gesture → in-session pause toggle (the Android counterpart of the desktop Esc, D53).
        // A host/session action handled OUTSIDE the deterministic `input_frame`, so the sim/checksum
        // stream is untouched. Drained as a one-shot edge so one press toggles exactly once.
        if input.take_back_pressed() {
            if let Some(game) = game.as_mut() {
                game.toggle_pause();
            }
        }

        if window.surface_up {
            if let (Some(rhi), Some(game)) = (rhi.as_mut(), game.as_mut()) {
                // Push the player's embodied look prefs (Compose Settings) into the engine's touch-look
                // seam — the Android twin of `pal-desktop::scale_look`. Android's drag-look delta is
                // produced *inside* `engine::touch_controls`, not at this PAL boundary, so the prefs
                // ride a `Game` setter instead of scaling `InputFrame.look_axis` (which the touch path
                // ignores). Host/presentation only — it never enters the deterministic sim or checksum
                // (invariants #1/#2). Cheap (two field writes); done each frame so a mid-match Settings
                // change would take effect immediately.
                game.set_touch_look_prefs(input.look_sensitivity(), input.invert_y());

                // Shell overlay buttons (pause / reconnect / post-match summary). A tap-up while an
                // overlay is up belongs to that overlay, not the match world: hit-test it in NDC and
                // either feed the resolved session action back to the shell (pause → Resume /
                // Surrender) or — for the terminal summary's DISMISS — finish the NativeActivity,
                // returning to the Compose title (the Android twin of the desktop ExitToTitle, D52).
                // Done BEFORE the world-input blanking below so the same release can't also drive a
                // selection underneath. Mirrors app/src/main.rs.
                if input_frame.pointer_up {
                    if let Some((px, py)) = input_frame.pointer {
                        let (w, h) = rhi.size();
                        // Shared pixel→NDC seam (engine; unit-tested) — desktop runs the same one, so
                        // the leave-to-title hit-test can't diverge across platforms (invariant #2).
                        let ndc = pixel_to_ndc(px, py, w, h);
                        match game.overlay_click(ndc) {
                            Some(OverlayClick::Session(action)) => {
                                game.apply_session_action(action);
                                input_frame.pointer_up = false;
                                input_frame.pointer_down = false;
                            }
                            Some(OverlayClick::Rematch) => {
                                // Re-seed a fresh match of the same scene/loadout. Deferred: `game`
                                // is borrowed here; the rebuild happens after this block (it needs to
                                // reassign the owned `game`/`campaign_*`). Mirrors the desktop
                                // `HostTransition::Rematch`.
                                pending_rematch = true;
                                input_frame.pointer_up = false;
                                input_frame.pointer_down = false;
                            }
                            Some(OverlayClick::Dismiss) => {
                                finish_activity(&app, campaign_result_code)
                            }
                            None => {}
                        }
                    }
                }
                // While a shell overlay (pause / reconnect / post-match summary) is up the match is
                // frozen underneath it: blank this frame's world-driving input so stray touches
                // behind the menu can't select units, fire the weapon, or pan the camera. Mirrors the
                // desktop host (app/src/main.rs). Single-player pause also halts the tick via
                // `halts_local_tick`; this stops *world input*, not the clock.
                if game.shell_overlay_active() {
                    input_frame = InputFrame::default();
                }
                let now = Instant::now();
                let dt = now.duration_since(last_frame).as_secs_f32();
                last_frame = now;
                let viewport = rhi.size();
                if let Some((frame, view)) = rhi.acquire() {
                    game.frame(
                        &input_frame,
                        dt,
                        viewport,
                        rhi.device(),
                        rhi.queue(),
                        &view,
                        &mut audio,
                        &thermal,
                    );
                    rhi.present(frame);

                    // Record-on-win (Compose parity C5): the first frame a campaign match reads
                    // `Won`, latch the packed Activity result code so the finish path reports the
                    // clear back to the Compose shell. `mission_status()` is a `&self` read of the
                    // host-side objective layer — safe now that `game.frame` (which took `&mut self`)
                    // has returned — and never folded into the sim/checksum (invariants #1/#7), so it
                    // cannot desync. Latched once; a loss/exit leaves it `None` and records nothing.
                    if let Some((node, tier)) = campaign_launch {
                        if campaign_result_code.is_none()
                            && game.mission_status() == MissionStatus::Won
                        {
                            campaign_result_code =
                                Some(crate::launch::campaign_result_code(node, tier));
                        }
                    }

                    // Heartbeat: count this presented frame, then ~once per second emit a
                    // single line with achieved FPS + the read-only sim tick/checksum + the
                    // current thermal bucket. The checksum read is `&self`, safe to call now
                    // that `game.frame` (which took `&mut self`) has returned; the thermal read
                    // is the same `&thermal` sensor we just passed into `game.frame`. Nothing
                    // here feeds back into the sim. The per-second `thermal <State>` field lets
                    // `scripts/android-fps.sh` track thermal *escalation across the window* (the
                    // D21 dual-rate datum) rather than only the one-time startup `thermal:` line.
                    frame_count += 1;
                    frames_since_report += 1;
                    let elapsed = now.duration_since(last_report);
                    if elapsed >= Duration::from_secs(1) {
                        use gonedark_pal::ThermalSensor;
                        let fps = frames_since_report as f32 / elapsed.as_secs_f32();
                        info!(
                            "heartbeat: {fps:.1} fps | frame {n} | tick {t} | thermal {therm} | checksum {c:016x}",
                            n = frame_count,
                            t = game.tick_count(),
                            therm = crate::thermal::thermal_label(thermal.thermal_state()),
                            c = game.checksum(),
                        );
                        frames_since_report = 0;
                        last_report = now;
                    }
                }
            }
        }

        // Deferred REMATCH: the post-match summary's REMATCH was tapped this frame. Rebuild a fresh
        // match of the same scene/loadout *in place* via the shared builder (identical to the first
        // boot), reusing the live device. Done here, after the overlay-click block released its
        // borrow of `game`, so we can reassign the owned `game`/campaign state. Only runs while the
        // surface (`rhi`) is up — otherwise the tap is dropped (there is no device to build against).
        if pending_rematch {
            pending_rematch = false;
            if let Some(r) = rhi.as_ref() {
                let (g, cl) = build_match_game(r, &launch);
                campaign_launch = cl;
                campaign_result_code = None;
                game = Some(g);
                last_frame = Instant::now();
            }
        }

        if window.destroy_requested {
            break 'outer;
        }
    }

    info!("android_main: clean exit");
}

/// Finish the `NativeActivity` so Android tears the match down and returns to the launcher (the
/// Compose `MainActivity` title) — the Android twin of the desktop ExitToTitle (D52), invoked when
/// the post-match summary's DISMISS is tapped (`OverlayClick::Dismiss`). The `MainEvent::Destroy`
/// that `Activity.finish()` triggers then breaks the run loop cleanly on the next poll.
///
/// When `result_code` is `Some` (a campaign mission was WON — Compose parity C5), it is handed back
/// via `Activity.setResult(int)` *before* `finish()` so the Compose shell's `ActivityResult`
/// callback can record the clear (the split-activity twin of the desktop host's single-process
/// record-on-win). `None` leaves the default `RESULT_CANCELED`, so a non-win return records nothing.
/// The pure packing lives in the host-tested [`crate::launch::campaign_result_code`].
///
/// Best-effort over JNI and **never fatal**: any attach/lookup failure is swallowed, and a pending
/// JVM exception is cleared so a failed call can't abort the process on the next JNI op — the same
/// discipline the thermal reader uses (see [`crate::thermal`]). This is un-constructible glue (no
/// real `JNIEnv`/`Activity` off a device), so it is exempt from unit coverage; the click→action
/// decision it serves (`overlay_click_action`) is host-tested in the `engine` crate.
fn finish_activity(app: &AndroidApp, result_code: Option<i32>) {
    use jni::objects::{JObject, JValue};
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: the pointers come from `android-activity`'s live `AndroidApp`, valid while the
    // activity is running (same handles the thermal reader attaches through).
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) };
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    // jni 0.22 attaches via a scoped closure handing back a borrowed `Env`, and JNI object refs are
    // tied to it — so the `Activity` handle and the `finish()` call both live inside the closure.
    // Any attach/lookup failure is swallowed (best-effort, never fatal).
    let _ = vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
        // SAFETY: `activity_ptr` is a live local ref android-activity owns for the call's duration.
        let activity = unsafe { JObject::from_raw(&*env, activity_ptr) };
        // A campaign win: Activity.setResult(int) : void — reported BEFORE finish() so the launcher's
        // ActivityResult callback receives it. A failed call clears the pending exception and we
        // still fall through to finish() (fail-safe: the match must still tear down).
        if let Some(code) = result_code {
            if env
                .call_method(&activity, jni_str!("setResult"), jni_sig!("(I)V"), &[JValue::Int(code)])
                .is_err()
            {
                env.exception_clear();
            }
        }
        // Activity.finish() : void. On any failure, clear the pending exception so we fail safe.
        if env
            .call_method(&activity, jni_str!("finish"), jni_sig!("()V"), &[])
            .is_err()
        {
            env.exception_clear();
        }
        Ok(())
    });
}

/// Read the Compose shell's launch-config `Intent` extra ([`crate::launch::EXTRA_KEY`]) off the live
/// Build the shared [`Game`] for a match from the live device (`rhi`) and the Compose shell's
/// [`LaunchConfig`](crate::launch::LaunchConfig) — the scene, gunsmith loadout, army, accessibility
/// cues, and (for a campaign node) the replay-tier tuning. Returns the game plus the
/// `campaign_launch` marker (`Some((node, tier))` for a campaign scene, `None` otherwise) so the
/// caller can record a clear on a win. Shared by the initial surface-creation path **and** the
/// post-match REMATCH so the two can never drift (they used to be one inline block; a rematch that
/// re-seeded differently would be a subtle desync from the first boot). Everything here is
/// deterministic match-setup applied before tick 0 (checksum-neutral, invariants #1/#2/#7); the same
/// `DEFAULT_SEED` as desktop keeps the scene bit-identical. Un-constructible glue off a device (needs
/// a real `AndroidRhi`), so it's exempt from unit coverage — the seams it composes
/// (`Game::new_scene_with_loadout`, `select_army`, `apply_campaign_tuning`) are host-tested in engine.
fn build_match_game(
    rhi: &AndroidRhi,
    launch: &crate::launch::LaunchConfig,
) -> (Game, Option<(u32, u8)>) {
    // Unknown scene token → the real playable match (Skirmish), matching the desktop default boot.
    let scene = Scene::parse(&launch.scene).unwrap_or(Scene::Skirmish);
    // The wire's gunsmith indices (already clamped 0..=2 by the parser) index the slot enums' ALL
    // order. `Loadout::STANDARD` (all-zero) reproduces the pre-parity boot byte-for-byte. Stock/Muzzle
    // aren't on the launch wire yet (D85) — default them to Standard.
    let loadout = Loadout {
        optic: Optic::ALL[launch.optic as usize],
        barrel: Barrel::ALL[launch.barrel as usize],
        magazine: Magazine::ALL[launch.magazine as usize],
        ..Loadout::STANDARD
    };
    let mut game =
        Game::new_scene_with_loadout(rhi.device(), rhi.format(), DEFAULT_SEED, scene, loadout);
    // Field the player's picked army through the SHARED `Game::select_army` → `core::shell` seam
    // (never a per-platform fork, invariant #2). The parser guarantees a valid ordinal; the guarded
    // lookup can never index past `Army::ALL`.
    let army = Army::ALL
        .get(launch.army as usize)
        .copied()
        .unwrap_or(Army::Us);
    game.select_army(Faction::Player, army);
    // Accessibility cues (CVD text labels + visual sound echoes) — host/presentation only, never the
    // sim or checksum (invariants #1/#4/#6). WS-D palette parity isn't on the wire yet → default hue.
    game.set_accessibility_prefs(
        launch.colorblind_cues,
        launch.visual_sound_cues,
        gonedark_engine::PaletteMode::Off,
    );
    // Campaign-launch path (Compose parity C4): resolve the node through the SHARED registry seam and
    // apply the chosen replay tier's combat tuning via the SHARED `apply_campaign_tuning` (D83 — both
    // the 4→3 commander band and the situation modifiers), exactly as the desktop host does. Both
    // campaign scenes take this path (Seize → Mission1, gated Hold → Mission2).
    let mut campaign_launch = None;
    if matches!(scene, Scene::Mission1 | Scene::Mission2) {
        let node = NodeId(launch.node);
        let campaign = gonedark_engine::mission_registry::default_campaign();
        let registry = gonedark_engine::mission_registry::default_registry();
        if registry.resolve_node(&campaign, node).is_some() {
            let tier = Difficulty::from_tier(launch.diff).unwrap_or(Difficulty::Recruit);
            game.apply_campaign_tuning(tier);
        }
        campaign_launch = Some((launch.node, launch.diff));
    }
    (game, campaign_launch)
}

/// `NativeActivity` and parse it (Compose shell parity, Tier 0). Calls `activity.getIntent()` then
/// `intent.getStringExtra(KEY)` over JNI — the same attach-and-call discipline as
/// [`finish_activity`]. **Best-effort and never fatal:** any attach/lookup failure, a missing
/// intent, or an absent extra is swallowed and yields a default [`crate::launch::LaunchConfig`]
/// (Scene::Skirmish); a pending JVM exception is cleared so a failed call can't abort the process on
/// the next JNI op. This is un-constructible glue (no real `JNIEnv`/`Activity`/`Intent` off a
/// device), so it is exempt from unit coverage; the pure codec it feeds
/// ([`crate::launch::parse_launch_config`]) is exhaustively host-tested in `crate::launch`.
fn read_launch_config(app: &AndroidApp) -> crate::launch::LaunchConfig {
    use jni::objects::{JObject, JString};
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: the pointers come from `android-activity`'s live `AndroidApp`, valid while the
    // activity is running (the same handles `finish_activity`/the thermal reader attach through).
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) };
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    let mut wire = String::new();
    // jni 0.22 attaches via a scoped closure handing back a borrowed `Env`; the `Activity`/`Intent`
    // refs and every call live inside it. Any failure is swallowed (best-effort, never fatal) and
    // leaves `wire` empty → a default config.
    let _ = vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
        // SAFETY: `activity_ptr` is a live local ref android-activity owns for the call's duration.
        let activity = unsafe { JObject::from_raw(&*env, activity_ptr) };
        // Intent intent = activity.getIntent();
        let intent = match env
            .call_method(
                &activity,
                jni_str!("getIntent"),
                jni_sig!("()Landroid/content/Intent;"),
                &[],
            )
            .and_then(|v| v.l())
        {
            Ok(obj) => obj,
            Err(_) => {
                env.exception_clear();
                return Ok(());
            }
        };
        if intent.is_null() {
            return Ok(());
        }
        // String s = intent.getStringExtra(EXTRA_KEY);
        let key = env.new_string(crate::launch::EXTRA_KEY)?;
        let extra = match env
            .call_method(
                &intent,
                jni_str!("getStringExtra"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                &[(&key).into()],
            )
            .and_then(|v| v.l())
        {
            Ok(obj) => obj,
            Err(_) => {
                env.exception_clear();
                return Ok(());
            }
        };
        if extra.is_null() {
            return Ok(()); // no extra on the intent — default config
        }
        // getStringExtra returns an `Object`; checked-cast it to a `JString` (jni 0.22's
        // `cast_local`), then read it as a Rust `String` (`try_to_string`). A failed cast/read is
        // swallowed → default config.
        let jstr = match env.cast_local::<JString>(extra) {
            Ok(s) => s,
            Err(_) => {
                env.exception_clear();
                return Ok(());
            }
        };
        if let Ok(s) = jstr.try_to_string(&env) {
            wire = s;
        }
        Ok(())
    });
    crate::launch::parse_launch_config(&wire)
}

// ---------------------------------------------------------------------------------------
// Window — wraps AndroidApp lifecycle + native window dimensions.
// ---------------------------------------------------------------------------------------

/// [`Window`] backed by `android-activity`. Android owns the surface lifecycle, so "should
/// close" tracks the `Destroy` lifecycle event rather than a user-closeable window.
pub struct AndroidWindow {
    app: AndroidApp,
    width: u32,
    height: u32,
    /// True between `InitWindow` and `TerminateWindow` — i.e. a valid surface exists.
    surface_up: bool,
    destroy_requested: bool,
}

impl AndroidWindow {
    pub fn new(app: AndroidApp) -> Self {
        AndroidWindow {
            app,
            width: 0,
            height: 0,
            surface_up: false,
            destroy_requested: false,
        }
    }
}

impl Window for AndroidWindow {
    fn size(&self) -> (u32, u32) {
        // Prefer the live native-window dimensions; fall back to the cached values.
        if let Some(nw) = self.app.native_window() {
            (nw.width() as u32, nw.height() as u32)
        } else {
            (self.width, self.height)
        }
    }

    fn should_close(&self) -> bool {
        self.destroy_requested
    }

    fn pump(&mut self) -> bool {
        // The real pumping happens in `android_main`'s `poll_events`; this exists so `app`'s
        // run loop, if it drives the Window trait, gets a consistent close signal.
        !self.destroy_requested
    }
}

// ---------------------------------------------------------------------------------------
// Input — maps android-activity touch + key events onto pal::InputFrame.
// ---------------------------------------------------------------------------------------

/// [`Input`] backed by `android-activity` motion/key events. Translates Android's native
/// scheme into the engine's platform-agnostic [`InputFrame`] intent vocabulary
/// (platforms.md §5) — the core never sees a raw touch.
///
/// Mapping: in the **command view** a single touch sets `pointer` + `pointer_down` for the
/// tap/select/command scheme (D43) and a two-finger tap embodies. While **embodied**, every motion
/// event forwards the full down-pointer set as [`InputFrame::touches`]; the engine's pure
/// `touch_controls` seam turns those into the on-screen FPS HUD intents (move stick + drag-look +
/// Fire/Crouch/Reload/Surface). Gyro aim is a later optional aid (`TODO(phase3+)`).
pub struct AndroidInput {
    app: AndroidApp,
    frame: InputFrame,
    /// True between the down and up of a MULTI-finger gesture (the two-finger embody tap, D43). It
    /// suppresses the single-finger tap's `pointer_up` release latch on lift, so lifting from a
    /// two-finger gesture doesn't also resolve a spurious empty-ground command/selection.
    multi_touch: bool,
    /// Edge flag: the **system back gesture/button** was pressed since the last drain. It is the
    /// Android counterpart of the desktop **Esc** pause key ([D53](../../docs/decisions.md)) — a
    /// host/session concern, deliberately kept OUT of [`InputFrame`] (which feeds the deterministic
    /// sim) so the checksum stream is untouched. The host drains it via [`Self::take_back_pressed`]
    /// after `poll` and routes it to `Game::toggle_pause`, exactly as the desktop host handles Esc
    /// outside the sim keymap.
    back_pressed: bool,
    /// Player look prefs from the Compose shell's Settings, pushed by the host via
    /// [`set_look_prefs`](Self::set_look_prefs) after the launch config is read (host/presentation
    /// only, never the sim — invariants #1/#2, exactly as `pal-desktop` keeps them out of the
    /// deterministic input). `look_sensitivity` is a multiplier (`1.0` = stock); `invert_y` flips the
    /// embodied pitch axis. Default `1.0` / `false` (a stock pass-through).
    ///
    /// APPLIED via [`look_sensitivity`](Self::look_sensitivity) / [`invert_y`](Self::invert_y): the
    /// host reads these each frame and pushes them into the engine's touch-look seam with
    /// `Game::set_touch_look_prefs`, which scales the drag-look delta produced inside
    /// `engine::touch_controls::TouchControls::update` — the Android twin of `pal-desktop::scale_look`
    /// (desktop scales its raw mouse delta at the PAL boundary; Android's delta is produced in the
    /// engine, so the prefs are handed down to that seam). Host/presentation only, never the sim
    /// (invariants #1/#2).
    look_sensitivity: f32,
    invert_y: bool,
}

impl AndroidInput {
    pub fn new(app: AndroidApp) -> Self {
        AndroidInput {
            app,
            frame: InputFrame::default(),
            multi_touch: false,
            back_pressed: false,
            look_sensitivity: 1.0,
            invert_y: false,
        }
    }

    /// Set the player's embodied look prefs (Compose shell Settings). `sensitivity` is the look
    /// multiplier decoded from the launch config's ×100 wire value via
    /// [`crate::launch::sens_x100_to_f32`] (`1.0` = stock); `invert_y` flips the embodied pitch axis.
    /// The host calls this once after reading the launch config. The mirror of
    /// `pal-desktop::set_look_prefs`.
    ///
    /// Desktop scales the raw *mouse-look delta* at drain time (`pal-desktop::scale_look`) because that
    /// delta is produced at the desktop PAL boundary. On Android the embodied look delta is **not**
    /// produced here: this backend forwards the raw down-finger set (`capture_touches` →
    /// [`InputFrame::touches`]), and the engine's pure `touch_controls::TouchControls::update` seam
    /// derives the drag-look `look_delta` from those raw finger *positions* inside the engine —
    /// `input.look_axis` is ignored while embodied. Scaling the raw touch coordinates here would
    /// corrupt the stick centres and button hit-tests (position-based, not delta-based), so instead the
    /// prefs are stored and the host pushes them into that engine seam once per frame with
    /// [`Game::set_touch_look_prefs`], which scales + pitch-flips the derived `look_delta`. This method
    /// is the mirror of `pal-desktop::DesktopInput::set_look_prefs`; the getters
    /// [`look_sensitivity`](Self::look_sensitivity) / [`invert_y`](Self::invert_y) feed the engine.
    pub fn set_look_prefs(&mut self, sensitivity: f32, invert_y: bool) {
        self.look_sensitivity = sensitivity;
        self.invert_y = invert_y;
    }

    /// The stored look-sensitivity multiplier (`1.0` = stock) — the host pushes it into the engine's
    /// touch-look seam each frame via `Game::set_touch_look_prefs` (see [`set_look_prefs`](Self::set_look_prefs)).
    pub fn look_sensitivity(&self) -> f32 {
        self.look_sensitivity
    }

    /// Whether the player has enabled pitch-invert for the embodied look — handed to the engine's
    /// touch-look seam alongside [`look_sensitivity`](Self::look_sensitivity).
    pub fn invert_y(&self) -> bool {
        self.invert_y
    }

    /// Take (and clear) the pending **back-gesture** edge — the host calls this once per loop after
    /// [`poll`](Input::poll) and, if set, toggles the in-session pause overlay
    /// (`Game::toggle_pause`, [D53](../../docs/decisions.md)). A session action, never a sim input:
    /// it does not enter the [`InputFrame`], so the deterministic tick/checksum is unaffected.
    pub fn take_back_pressed(&mut self) -> bool {
        core::mem::take(&mut self.back_pressed)
    }

    /// Translate one motion (touch) event into the running InputFrame (the command-layer touch
    /// scheme, D43). `multi_touch` tracks whether a multi-finger gesture is in flight so its lift
    /// doesn't latch a spurious single-finger release.
    ///
    /// Gesture grammar (mirrors the desktop classic-RTS split via the shared intent vocabulary):
    /// one finger down/move/up drives `pointer_down` + the `pointer_up` release edge — the engine's
    /// `Selection` then resolves it to a tap-select, a band-select, or (off a unit, with a
    /// selection) a Move/Attack via the `command_tap` mode (set in [`Self::poll`]). A two-finger tap
    /// toggles embodiment.
    fn apply_motion(frame: &mut InputFrame, multi_touch: &mut bool, motion: &MotionEvent) {
        // Forward the full currently-down pointer set EVERY motion event: while embodied the engine's
        // `touch_controls` seam reads `frame.touches` to drive the on-screen FPS HUD (move stick +
        // drag-look + Fire/Crouch/Reload/Surface). The command layer below still uses the primary
        // pointer for taps; the two layers read the same touches, never fork (invariant #2).
        capture_touches(frame, motion);

        // android-activity 0.6 exposes pointers via `motion.pointers()`; the primary pointer
        // drives the command-layer tap position.
        let action = motion.action();
        if let Some(p) = motion.pointers().next() {
            frame.pointer = Some((p.x(), p.y()));
        }
        // Fingers down for this event (a PointerDown includes the newly-landed one).
        let pointer_count = motion.pointers().count();
        match action {
            MotionAction::Down | MotionAction::PointerDown => {
                if pointer_count >= 2 {
                    // TWO-FINGER TAP = EMBODY (command view only, by construction): raise ONLY
                    // `embody_pressed`. `map_input_commands` no-ops it when already embodied
                    // (`embody_pressed && !embodied`), so this gesture is harmless while possessed —
                    // where two fingers now mean move+look on the twin-stick HUD. Surfacing is the
                    // on-screen Surface BUTTON (engine `touch_controls`), NOT a gesture, so the two
                    // never collide. Mark multi-finger + drop the single-finger down (no select/cmd).
                    frame.embody_pressed = true;
                    frame.pointer_down = false;
                    *multi_touch = true;
                } else {
                    frame.pointer_down = true;
                }
            }
            // The last finger lifted (Up), or the gesture was cancelled. Drop the held state and —
            // for a genuine SINGLE-finger gesture — latch the `pointer_up` release so `Selection`
            // resolves the tap/drag this frame. (Without this latch the command layer never
            // resolves on touch at all.) A multi-finger gesture suppresses the latch and resets.
            MotionAction::Up | MotionAction::Cancel => {
                frame.pointer_down = false;
                if *multi_touch {
                    *multi_touch = false;
                } else {
                    frame.pointer_up = true;
                }
            }
            // A secondary finger lifted while others remain: still part of the multi-finger gesture,
            // so no single-tap release. Keep `multi_touch` set until the final Up.
            MotionAction::PointerUp => {}
            // Move keeps the current down-state; pointer position already updated above.
            _ => {}
        }
        // Embodied locomotion/look/fire/crouch/reload IS now wired: `capture_touches` forwards the
        // raw pointers and the engine's pure, host-tested `touch_controls` seam turns them into
        // `move_axis`/`look_axis`/`fire` + the Crouch/Reload/Surface button edges (the seam lives in
        // `engine`, not here, because an Android `MotionEvent` can't be unit-tested).
        // TODO(phase3+): gyro (ndk Sensor API) -> look_axis as an optional aim aid.
        // TODO(phase2+): the command-view PRODUCTION intents — `building_slot` (place a Camp at the
        //   tapped point), `train_slot` (queue a unit at the active camp), `upgrade_pressed` (upgrade
        //   it) — join that deferred on-screen-UI slice: each is an on-screen palette/panel button +
        //   hit-test latched onto the matching InputFrame edge, exactly as desktop binds B/R/H/U. The
        //   engine already consumes these (`engine::{build_ui,train_ui,upgrade_ui}`); until the touch
        //   buttons exist they stay default (`None`/`false`), so Android simply doesn't build/train/
        //   upgrade yet — like `command_slot`, which is likewise unset here pending the radial UI.
    }

    /// Translate one key event into intents. The **system back** key routes to the host-side
    /// `back_pressed` edge (the pause toggle, [D53](../../docs/decisions.md)) — NOT into the
    /// deterministic [`InputFrame`]; everything else (gamepad face buttons) is still unmapped.
    fn apply_key(frame: &mut InputFrame, back_pressed: &mut bool, key: &KeyEvent) {
        // Edge-triggered: only act on the Down edge (one toggle per physical press).
        if key.action() == KeyAction::Down {
            if key.key_code() == Keycode::Back {
                // Back = open/close the in-session pause overlay (the Android counterpart of the
                // desktop Esc, D53). Routed to the host via `take_back_pressed`, never to the sim
                // input frame, so the per-tick checksum is untouched (invariants #1/#4). We consume
                // it (the poll loop returns `Handled`), so back never falls through to the OS's
                // default "finish the activity" — leaving the match is the pause menu's job.
                *back_pressed = true;
                return;
            }
            // TODO(phase1-step6): map a gamepad/keyboard "embody" / "surface" / "fire" key here once
            //   the chosen bindings exist. Left intentionally unmapped — picking bindings is a design
            //   call, not to be silently decided.
            let _ = frame; // placeholder to keep the signature honest
        }
    }
}

/// Rebuild [`InputFrame::touches`] from a motion event's **currently-down** pointers, so the
/// engine's `touch_controls` seam sees the live finger set each frame. The pointer that is *leaving*
/// on this event is excluded: `Up` removes the last finger, `PointerUp` a secondary one (the action
/// pointer at [`MotionEvent::pointer_index`]), `Cancel` removes all. Capped at [`MAX_TOUCHES`] (a
/// twin-stick + a couple of buttons never needs more). The id is the platform `pointer_id`, stable
/// across frames for a held finger so the seam can track which control each finger owns.
fn capture_touches(frame: &mut InputFrame, motion: &MotionEvent) {
    if motion.action() == MotionAction::Cancel {
        frame.touch_count = 0;
        return;
    }
    let lifting = match motion.action() {
        MotionAction::Up | MotionAction::PointerUp => Some(motion.pointer_index()),
        _ => None,
    };
    let mut n = 0usize;
    for (idx, p) in motion.pointers().enumerate() {
        if Some(idx) == lifting {
            continue;
        }
        if n >= MAX_TOUCHES {
            break;
        }
        frame.touches[n] = TouchSample {
            id: p.pointer_id() as u64,
            x: p.x(),
            y: p.y(),
        };
        n += 1;
    }
    frame.touch_count = n as u8;
}

impl Input for AndroidInput {
    fn poll(&mut self) -> InputFrame {
        // Reset edge-triggered intents each poll; keep level state (pointer/pointer_down)
        // across frames so a held touch stays down.
        self.frame.embody_pressed = false;
        self.frame.surface_pressed = false;
        self.frame.fire = false;
        // The pointer-release is an EDGE (one frame), like the *_pressed intents — clear it each
        // poll so a single lift resolves exactly one selection/command (D43).
        self.frame.pointer_up = false;
        // The back-gesture pause edge is likewise one-shot — clear it so a single press toggles the
        // pause overlay exactly once (the host drains it via `take_back_pressed` after this poll).
        self.back_pressed = false;
        // Touch is the single-pointer "tap commands" scheme (D43): a tap off a unit, with a
        // selection, issues the default order rather than deselecting. It's a mode, set every poll.
        self.frame.command_tap = true;

        // Drain the current native input batch. android-activity 0.6 hands input via an
        // iterator obtained from the app; we fold each event into `self.frame`.
        // NOTE: the exact call is `app.input_events_iter()` in 0.6 (returns a Result of an
        // iterator yielding `InputEvent`s). Confirm against the pinned crate on real
        // toolchain — older 0.5 used the `input_events(|e| ...)` closure form.
        if let Ok(mut iter) = self.app.input_events_iter() {
            // `next(&mut self, callback)`-style draining: process until the iterator is
            // exhausted, returning Handled so android-activity advances the queue. Split-borrow the
            // two fields the motion path mutates so the closure holds them disjointly from `app`.
            let frame = &mut self.frame;
            let multi_touch = &mut self.multi_touch;
            let back_pressed = &mut self.back_pressed;
            while iter.next(|event| {
                match event {
                    InputEvent::MotionEvent(motion) => {
                        Self::apply_motion(frame, multi_touch, motion)
                    }
                    InputEvent::KeyEvent(key) => Self::apply_key(frame, back_pressed, key),
                    // TextEvent and any future variants: ignored for the game input path.
                    _ => {}
                }
                InputStatus::Handled
            }) {}
        }

        self.frame.clone()
    }
}

// ---------------------------------------------------------------------------------------
// RHI — wgpu surface created from the ANativeWindow.
// ---------------------------------------------------------------------------------------

/// The Android wgpu surface + device, which selects the **Vulkan** backend automatically
/// (platforms.md §3). The surface is created from the `ANativeWindow` handed to us on
/// `InitWindow`; it must be recreated whenever the window is recreated (resume).
///
/// This is the Android mirror of `pal-desktop::DesktopRenderSurface`: it exposes concrete
/// `device()`/`queue()`/`format()`/`acquire()`/`present()` accessors that the shared
/// `engine::Game` (via `android_main`) drives — the abstract `pal::Rhi` trait is not
/// implemented here (D19: the device crosses at the concrete wiring layer).
pub struct AndroidRhi {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

/// A `'static` raw-handle wrapper over the `ndk` `NativeWindow`. wgpu's
/// `create_surface` needs a `HasWindowHandle + HasDisplayHandle`; the `ndk` native window
/// provides the Android raw window handle. We keep the `NativeWindow` alive for as long as
/// the surface uses it (here: the lifetime of `AndroidRhi`, dropped on TerminateWindow).
struct AndroidSurfaceTarget {
    native_window: ndk::native_window::NativeWindow,
}

impl HasWindowHandle for AndroidSurfaceTarget {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // ndk 0.9 implements raw-window-handle 0.6 for NativeWindow.
        self.native_window.window_handle()
    }
}

impl HasDisplayHandle for AndroidSurfaceTarget {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // Android uses no separate display handle.
        Ok(raw_window_handle::DisplayHandle::android())
    }
}

impl AndroidRhi {
    /// Create the wgpu device + surface from the live `ANativeWindow`.
    ///
    /// NOTE: this uses `pollster::block_on`-style synchronous adapter/device request via a
    /// tiny hand-rolled executor to avoid pulling in `pollster`. On Android, `request_*`
    /// futures resolve immediately on the calling thread in practice; if the pinned wgpu 29
    /// requires a real executor here, add `pollster` to the android deps.
    fn new(
        app: &AndroidApp,
        native_window: ndk::native_window::NativeWindow,
    ) -> Result<Self, String> {
        let _ = app; // reserved: AAssetManager etc. flow through `app` later.

        let (width, height) = (native_window.width() as u32, native_window.height() as u32);
        let target = AndroidSurfaceTarget { native_window };

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // wgpu picks Vulkan on Android; pin it explicitly so we never silently fall
            // back to GL. wgpu 29's `InstanceDescriptor` has no `Default` (the `display`
            // field is a boxed trait object), so spread from its constructor instead.
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        // SAFETY/lifetime: `target` owns the NativeWindow for the surface's life. We leak
        // the target into the surface's `'static` by boxing+forgetting through Arc so the
        // ANativeWindow stays valid until `AndroidRhi` (and thus the surface) is dropped on
        // TerminateWindow. The Arc is held by the closure below.
        let target = Arc::new(target);
        info!("RHI: instance created (Vulkan); native window {width}x{height}");
        let surface = instance
            .create_surface(target.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        info!("RHI: surface created");

        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("request_adapter: {e}"))?;
        let ainfo = adapter.get_info();
        info!(
            "RHI: adapter = {} (backend {:?}, type {:?}, driver {})",
            ainfo.name, ainfo.backend, ainfo.device_type, ainfo.driver
        );

        // Vulkan 1.1 mobile floor (platforms.md §6): keep the conservative downlevel limits
        // for every resource EXCEPT texture dimensions. downlevel_defaults caps
        // max_texture_dimension_2d at 2048, but a modern phone's swapchain is wider (this
        // Adreno is 2340px) — configuring a 2340-wide surface against a 2048 cap is a
        // validation error. Raise just the texture-dimension caps to the adapter's real max
        // so the full-screen surface configures. (wgpu 29 dropped Limits::using_resolution,
        // so set the three fields by hand.)
        let adapter_limits = adapter.limits();
        let mut required_limits = wgpu::Limits::downlevel_defaults();
        required_limits.max_texture_dimension_1d = adapter_limits.max_texture_dimension_1d;
        required_limits.max_texture_dimension_2d = adapter_limits.max_texture_dimension_2d;
        required_limits.max_texture_dimension_3d = adapter_limits.max_texture_dimension_3d;

        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("gonedark-android-device"),
            required_features: wgpu::Features::empty(),
            required_limits,
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("request_device: {e}"))?;
        info!("RHI: device + queue created");

        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() {
            return Err("surface reported no supported formats for this adapter".to_string());
        }
        info!(
            "RHI: surface caps — formats={:?} present_modes={:?} alpha_modes={:?}",
            caps.formats, caps.present_modes, caps.alpha_modes
        );
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        // `surface.configure` reports failure via wgpu's uncaptured-error channel (a fatal
        // panic by default), not a Result — wrap it in a validation scope so a bad config is a
        // readable Err here instead of an opaque crash.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        if let Some(err) = block_on(scope.pop()) {
            return Err(format!(
                "surface.configure ({format:?} {width}x{height}): {err}"
            ));
        }
        info!("RHI: surface configured {width}x{height} as {format:?}");

        // Keep the surface target alive for the surface's lifetime by forgetting the Arc;
        // it is reclaimed implicitly when the process tears down the native window. (We
        // drop the whole RHI on TerminateWindow, which is the real lifetime boundary.)
        std::mem::forget(target);

        Ok(AndroidRhi {
            _instance: instance,
            surface,
            device,
            queue,
            config,
        })
    }

    /// The wgpu device — handed to `engine::Game::new`/`frame` (D19: the device crosses at
    /// this concrete wiring layer, never through the abstract `pal` trait).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Reconfigure the swapchain on resize (ignore zero-area).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Acquire the next swapchain image + a default view, mirroring
    /// `DesktopRenderSurface::acquire`. `None` if the surface is lost/outdated (reconfigure +
    /// skip the frame); the caller recreates next frame. wgpu 29 returns a
    /// `CurrentSurfaceTexture` enum, not a `Result`.
    pub fn acquire(&mut self) -> Option<(wgpu::SurfaceTexture, wgpu::TextureView)> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                Some((frame, view))
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                warn!("get_current_texture: {other:?}");
                None
            }
        }
    }

    /// Present a previously acquired frame.
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        frame.present();
    }
}

// ---------------------------------------------------------------------------------------
// Audio (real, oboe/AAudio — D29) + Storage (still a stub).
// ---------------------------------------------------------------------------------------

/// [`Audio`] backed by a low-latency **AAudio** stream via `oboe` (platforms.md §2, D29).
///
/// The Android mirror of `pal-desktop::DesktopAudio`: it renders the SAME positioned mix
/// (`engine::audio::mix_cues` → [`AudioCue`](gonedark_pal::AudioCue)s) the desktop backend does, by
/// pushing each cue through the shared, host-tested `gonedark_pal::mix` render math (equal-power
/// pan from `azimuth`, gain clamp, the `muffled` low-pass that makes off-map bleed read as distant
/// — invariant #6 — voice summation + soft-clamp + [`MAX_VOICES`](gonedark_pal::mix::MAX_VOICES)
/// eviction). Only the oboe stream lifecycle + the realtime `on_audio_ready` callback live here;
/// all the math is in `pal::mix`, so this glue is the thin, host-un-constructible part.
///
/// Invariant #8 / audio-never-load-bearing: if the device/stream can't be opened the sink degrades
/// to a silent no-op (`inner: None`, logged to logcat via `log::warn!`) — it NEVER panics.
///
/// # NOT device-verified
/// The oboe builder/callback calls are written against the pinned `oboe` 0.6 API but the audible
/// output, the negotiated low-latency path (§2), and the muffled-bleed audibility are device-
/// judgment calls — shake them out with `pnpm android:dev` (listen + read logcat for the
/// `[audio] disabled (silent)` fallback line).
pub struct AndroidAudio {
    inner: Option<AndroidAudioActive>,
    /// Player volume prefs (`[0, 1]`) from the Compose shell's Settings, pushed by the host via
    /// [`set_gains`](Self::set_gains) once the launch config is read. Default `1.0` (a pass-through)
    /// so a backend nobody configures (the startup sanity `play_oneshot`, tests) renders the unscaled
    /// mix. Applied per cue at queue time through the shared [`scaled_gain`] seam — the exact mirror of
    /// `pal-desktop::audio::DesktopAudio`.
    master: f32,
    sfx: f32,
}

/// The live oboe output stream (kept alive by ownership), the shared mixer the realtime callback
/// reads, and the synthesized cue bank the game thread looks voices up in.
struct AndroidAudioActive {
    // The async stream owns the callback (which holds a clone of `mixer`). Kept alive by
    // ownership; dropping it stops + closes the stream. Boxed behind the concrete oboe type.
    _stream: oboe::AudioStreamAsync<oboe::Output, OboeMixCallback>,
    mixer: Arc<Mutex<Mixer>>,
    bank: HashMap<SoundId, Arc<Vec<f32>>>,
}

/// The realtime audio callback: it owns a handle to the shared [`Mixer`] and fills each requested
/// stereo frame buffer. It must NEVER block the audio thread (oboe docs: no locks/alloc/syscalls in
/// `on_audio_ready`), so it `try_lock`s the mixer and emits silence if the game thread holds it —
/// exactly the desktop cpal callback's rule.
struct OboeMixCallback {
    mixer: Arc<Mutex<Mixer>>,
}

impl oboe::AudioOutputCallback for OboeMixCallback {
    // Stereo f32 frames: the frame type's element is `(f32, f32)` (left, right).
    type FrameType = (f32, oboe::Stereo);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn oboe::AudioOutputStreamSafe,
        frames: &mut [(f32, f32)],
    ) -> oboe::DataCallbackResult {
        match self.mixer.try_lock() {
            Ok(mut m) => {
                for frame in frames.iter_mut() {
                    let (l, r) = m.next_frame();
                    *frame = (l, r);
                }
            }
            Err(_) => {
                // Game thread holds the lock (its critical section is tiny) — emit a frame of
                // silence rather than block the realtime thread.
                for frame in frames.iter_mut() {
                    *frame = (0.0, 0.0);
                }
            }
        }
        oboe::DataCallbackResult::Continue
    }
}

impl Default for AndroidAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidAudio {
    /// Open the AAudio output stream; on any failure degrade to a silent no-op (invariant #8).
    pub fn new() -> Self {
        let inner = match AndroidAudioActive::open() {
            Ok(active) => Some(active),
            Err(e) => {
                warn!("[audio] disabled (silent): {e}");
                None
            }
        };
        AndroidAudio {
            inner,
            master: 1.0,
            sfx: 1.0,
        }
    }

    /// Set the master / SFX volume prefs (`[0, 1]`, validated at the Compose shell's Settings
    /// boundary and decoded from the launch config's integer percents via
    /// [`crate::launch::pct_to_gain`]). The host calls this once after reading the launch config;
    /// cues queued afterwards are scaled by `master * sfx`. The exact mirror of
    /// `pal-desktop::audio::DesktopAudio::set_gains`.
    pub fn set_gains(&mut self, master: f32, sfx: f32) {
        self.master = master;
        self.sfx = sfx;
    }

    /// Queue one voice for `sound`, panned by `azimuth`, scaled by `gain` **and** the player's
    /// master/SFX volumes, low-passed when `muffled` — via the shared `gonedark_pal::mix` render math
    /// (identical to desktop). The pan/gain/muffle derivation is `voice_from_cue`; the player-volume
    /// scaling is the shared [`scaled_gain`], exactly as `pal-desktop::audio` does.
    fn queue(&self, sound: SoundId, azimuth: f32, gain: f32, muffled: bool) {
        let Some(active) = &self.inner else { return };
        let Some(samples) = active.bank.get(&sound) else {
            return;
        };
        let g = scaled_gain(gain, self.master, self.sfx);
        let voice = voice_from_cue(Arc::clone(samples), azimuth, g, muffled);
        if let Ok(mut mixer) = active.mixer.lock() {
            mixer.push(voice);
        }
    }
}

impl AndroidAudioActive {
    fn open() -> Result<AndroidAudioActive, String> {
        let mixer = Arc::new(Mutex::new(Mixer::new()));
        let callback = OboeMixCallback {
            mixer: Arc::clone(&mixer),
        };

        // Low-latency AAudio output (platforms.md §2): stereo f32, exclusive sharing + low-latency
        // performance mode is the lowest-latency AAudio path. We let oboe negotiate the device
        // sample rate (no `set_sample_rate`), then read it back to synthesize the cue bank at that
        // rate so cues play at the intended pitch.
        let mut stream = oboe::AudioStreamBuilder::default()
            .set_performance_mode(oboe::PerformanceMode::LowLatency)
            .set_sharing_mode(oboe::SharingMode::Exclusive)
            .set_format::<f32>()
            .set_stereo()
            .set_callback(callback)
            .open_stream()
            .map_err(|e| format!("oboe open_stream: {e:?}"))?;

        // The negotiated rate is known only after the stream opens; build the bank at it.
        let sample_rate = stream.get_sample_rate();
        let sample_rate = if sample_rate > 0 {
            sample_rate as u32
        } else {
            48_000 // defensive: a non-positive rate shouldn't happen, but never divide by it
        };
        let bank = synth_bank(sample_rate);

        stream
            .request_start()
            .map_err(|e| format!("oboe request_start: {e:?}"))?;
        info!("[audio] AAudio stream started ({sample_rate} Hz, stereo f32, low-latency)");

        Ok(AndroidAudioActive {
            _stream: stream,
            mixer,
            bank,
        })
    }
}

impl Audio for AndroidAudio {
    fn play_oneshot(&mut self, sound_id: u32) {
        // Legacy fire-and-forget path: map the opaque id onto a cue, centered at full gain.
        self.queue(oneshot_sound(sound_id), 0.0, 0.9, false);
    }

    fn submit_mix(&mut self, cues: &[gonedark_pal::AudioCue]) {
        // Render the per-frame positioned mix — pan by `cue.azimuth`, scale by `cue.gain`, low-pass
        // `cue.muffled` strategic bleed (invariant #6) — through the AAudio stream, exactly as
        // `pal-desktop::DesktopAudio` does, via the shared `pal::mix` math.
        for c in cues {
            self.queue(c.sound, c.azimuth, c.gain, c.muffled);
        }
    }
}

/// [`Storage`] stub. Real impl reads bundled assets via **AAssetManager** (from
/// `app.asset_manager()`) and writes user data under the app's files dir (POSIX +
/// AAsset, platforms.md §2).
pub struct AndroidStorage {
    app: AndroidApp,
}

impl AndroidStorage {
    pub fn new(app: AndroidApp) -> Self {
        AndroidStorage { app }
    }
}

impl Storage for AndroidStorage {
    fn read(&self, key: &str) -> Option<Vec<u8>> {
        // TODO(phase1-step6): resolve `key` against `self.app.asset_manager()` for bundled
        //   read-only assets, and against the internal files dir for user data. The
        //   AAssetManager handle is reachable via `self.app`.
        let _ = (&self.app, key);
        None
    }

    fn write(&mut self, key: &str, bytes: &[u8]) {
        // TODO(phase1-step6): write to the app's internal storage (files dir). Assets are
        //   read-only, so user writes go to the private data dir.
        let _ = (&self.app, key, bytes);
    }
}

// ---------------------------------------------------------------------------------------
// Tiny synchronous future driver (avoids a `pollster` dep for the few wgpu requests).
// ---------------------------------------------------------------------------------------

/// Block the current thread on a future. wgpu's adapter/device requests on the native
/// backends resolve without a real reactor, so a busy spin on the raw `poll` suffices.
/// If the pinned wgpu ever needs a real executor here, swap this for `pollster`.
fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    fn noop_waker() -> Waker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    // SAFETY: `fut` is owned and not moved after pinning.
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// INTEGRATOR NOTE (for the owner of app/Cargo.toml — NOT edited by this worker)
//
// `app` must gain, under its android target table, a dependency on this crate so the
// `android_main` symbol is linked into the cdylib that cargo-ndk builds:
//
//     [target.'cfg(target_os = "android")'.dependencies]
//     gonedark-pal-android = { path = "../pal-android" }
//
// `android_main` is exported FROM THIS CRATE (above). With a `cdylib`, the integrator
// builds the cdylib whose package depends on pal-android (so its symbols are retained) —
// in practice cargo-ndk targets the package that produces the loaded `.so`. The simplest
// wiring is to make `app` itself the cdylib (give `app` `crate-type = ["cdylib"]` on
// android and re-export: `#[cfg(target_os="android")] pub use gonedark_pal_android::*;`),
// OR target `gonedark-pal-android` directly with cargo-ndk. The android/README.md build
// command targets THIS crate's cdylib for the scaffold.
// ---------------------------------------------------------------------------------------
