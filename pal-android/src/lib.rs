//! Android PAL backend — `cargo-ndk` + `android-activity` + JNI shim (the ship target,
//! platforms.md §8).
//!
//! The entire crate is gated to `target_os = "android"`, so it compiles to an empty lib
//! on desktop/CI hosts and never drags Android deps into the host build. This file is the
//! structural realization of build-order step 6: the `android-activity` 0.6 entry point,
//! the lifecycle/window/input pump, the wgpu(Vulkan) surface bridge, and impls of the
//! `gonedark_pal` traits backed by Android.
//!
//! # NOT device-verified
//! This was written against the *pinned* `android-activity` 0.6 / `jni` 0.21 / `ndk` 0.9 /
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
#![cfg(target_os = "android")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gonedark_engine::{Game, DEFAULT_SEED};
use gonedark_pal::mix::{oneshot_sound, synth_bank, voice_from_cue, Mixer};
use gonedark_pal::{Audio, Input, InputFrame, SoundId, Storage, Window};

// Bring oboe's stream-control traits into scope for the methods used below (`get_sample_rate`
// lives on `AudioStreamBase`, `request_start` on `AudioStream`).
use oboe::{AudioStream, AudioStreamBase};

use android_activity::input::{InputEvent, KeyAction, KeyEvent, MotionAction, MotionEvent};
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
                                // Build the shared game against the live device. Same seed as
                                // desktop → the bit-identical deterministic scene.
                                game = Some(Game::new(
                                    new_rhi.device(),
                                    new_rhi.format(),
                                    DEFAULT_SEED,
                                ));
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
        let input_frame: InputFrame = input.poll();

        if window.surface_up {
            if let (Some(rhi), Some(game)) = (rhi.as_mut(), game.as_mut()) {
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
                    );
                    rhi.present(frame);

                    // Heartbeat: count this presented frame, then ~once per second emit a
                    // single line with achieved FPS + the read-only sim tick/checksum. The
                    // checksum read is `&self`, safe to call now that `game.frame` (which
                    // took `&mut self`) has returned. Nothing here feeds back into the sim.
                    frame_count += 1;
                    frames_since_report += 1;
                    let elapsed = now.duration_since(last_report);
                    if elapsed >= Duration::from_secs(1) {
                        let fps = frames_since_report as f32 / elapsed.as_secs_f32();
                        info!(
                            "heartbeat: {fps:.1} fps | frame {n} | tick {t} | checksum {c:016x}",
                            n = frame_count,
                            t = game.tick_count(),
                            c = game.checksum(),
                        );
                        frames_since_report = 0;
                        last_report = now;
                    }
                }
            }
        }

        if window.destroy_requested {
            break 'outer;
        }
    }

    info!("android_main: clean exit");
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
/// Phase 1 mapping (deliberately minimal — the real mobile control scheme is the Phase 0
/// product risk, roadmap.md): a single touch sets `pointer` + `pointer_down` for the
/// command-layer tap. Twin-stick / gyro for embodied combat is `TODO(phase1-step6+)`.
pub struct AndroidInput {
    app: AndroidApp,
    frame: InputFrame,
    /// True between the down and up of a MULTI-finger gesture (the two-finger embody tap, D43). It
    /// suppresses the single-finger tap's `pointer_up` release latch on lift, so lifting from a
    /// two-finger gesture doesn't also resolve a spurious empty-ground command/selection.
    multi_touch: bool,
}

impl AndroidInput {
    pub fn new(app: AndroidApp) -> Self {
        AndroidInput {
            app,
            frame: InputFrame::default(),
            multi_touch: false,
        }
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
                    // TWO-FINGER TAP toggles embodiment (D43): raise BOTH edge intents and let
                    // `engine::Game` resolve to embody-or-surface by the current state (the same
                    // resolution the desktop E/Q keys get). Mark the gesture multi-finger and drop
                    // the single-finger down so it neither selects nor commands.
                    frame.embody_pressed = true;
                    frame.surface_pressed = true;
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
        // TODO(phase2+): embodied locomotion — on-screen virtual sticks -> move_axis / look_axis
        //   while embodied, gyro (ndk Sensor API) -> look_axis; and the on-screen radial for the
        //   ADVANCED order vocabulary (long-press -> long_press + wedge hit-test -> command_slot).
        //   Those need on-screen UI + wedge hit-testing and are a separate slice (D43 "deferred").
        //   The stick-geometry -> axis mapping should be a pure free fn tested on the host (the seam
        //   pattern `engine::map_input_commands`/`Selection` already uses).
    }

    /// Translate one key event (back button, gamepad face buttons) into the InputFrame.
    fn apply_key(frame: &mut InputFrame, key: &KeyEvent) {
        // Edge-triggered: only set the *_pressed intents on the Down edge.
        if key.action() == KeyAction::Down {
            // TODO(phase1-step6): map a gamepad/keyboard "embody" / "surface" / "fire" key
            //   here once the chosen bindings exist. Left intentionally unmapped — picking
            //   bindings is a design call, not to be silently decided.
            let _ = frame; // placeholder to keep the signature honest
        }
    }
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
            while iter.next(|event| {
                match event {
                    InputEvent::MotionEvent(motion) => {
                        Self::apply_motion(frame, multi_touch, motion)
                    }
                    InputEvent::KeyEvent(key) => Self::apply_key(frame, key),
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
        match AndroidAudioActive::open() {
            Ok(active) => AndroidAudio {
                inner: Some(active),
            },
            Err(e) => {
                warn!("[audio] disabled (silent): {e}");
                AndroidAudio { inner: None }
            }
        }
    }

    /// Queue one voice for `sound`, panned by `azimuth`, scaled by `gain`, low-passed when
    /// `muffled` — via the shared `gonedark_pal::mix` render math (identical to desktop).
    fn queue(&self, sound: SoundId, azimuth: f32, gain: f32, muffled: bool) {
        let Some(active) = &self.inner else { return };
        let Some(samples) = active.bank.get(&sound) else {
            return;
        };
        let voice = voice_from_cue(Arc::clone(samples), azimuth, gain, muffled);
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
