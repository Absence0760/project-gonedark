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
//! `wgpu` 29 APIs, but **cannot be built for `aarch64-linux-android` on this workstation**
//! (no NDK, no `cargo-ndk`). A real machine with the NDK must do the for-target build and
//! on-device shakeout (see ../android/README.md). Spots that are deferred or that need an
//! API sanity-check on real toolchain are flagged with `TODO(...)` / `NOTE:` inline.
//!
//! # Where `android_main` lives
//! The exported entry point (`android_main`) lives **here**, not in `app`. `android-activity`
//! generates the actual JNI `ANativeActivity_onCreate` glue from the `#[no_mangle]`
//! `android_main` symbol via its `native-activity` feature. `app` (whose desktop `fn main`
//! owns the run loop on the host) must, for the Android target, depend on this crate and
//! re-export / route to this entry — see the integrator note at the bottom of this file.
#![cfg(target_os = "android")]

use std::sync::Arc;

use gonedark_pal::{Audio, Input, InputFrame, Rhi, Storage, Window};

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

    // Build the PAL pieces. Window/Input wrap the AndroidApp; the RHI is created lazily on
    // InitWindow (we have no surface until then).
    let mut window = AndroidWindow::new(app.clone());
    let mut input = AndroidInput::new(app.clone());
    let mut rhi: Option<AndroidRhi> = None;
    let storage = AndroidStorage::new(app.clone());
    let mut audio = AndroidAudio::default();

    // Sanity-touch the stub PAL services so the deferred impls are linked, not dead code.
    let _ = storage.read("settings");
    audio.play_oneshot(0);

    // The host owns the real sim/render loop (app/src/main.rs). For the scaffold we just
    // drive the lifecycle + a present-only RHI; wiring the actual `gonedark_core::Sim` +
    // `gonedark_render::Renderer` through here is build-order step 5/6 integration.
    // TODO(phase1-step6): construct the core Sim + Renderer and run the fixed-tick
    //   accumulator here, exactly as app/src/main.rs does for desktop, feeding `input.poll()`.

    'outer: loop {
        // Block forever when idle; android-activity wakes us on the next event. A real
        // frame loop would use a short/zero timeout while the window is up so it can
        // render continuously — kept conservative here.
        let timeout = if window.surface_up {
            Some(std::time::Duration::ZERO)
        } else {
            None
        };

        app.poll_events(timeout, |event| match event {
            PollEvent::Main(main_event) => match main_event {
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
                                rhi = Some(new_rhi);
                                info!("wgpu surface created at {w}x{h}");
                            }
                            Err(e) => warn!("RHI init failed: {e}"),
                        },
                        None => warn!("InitWindow with no native_window — skipping surface"),
                    }
                }
                MainEvent::TerminateWindow { .. } => {
                    info!("MainEvent::TerminateWindow — dropping surface");
                    window.surface_up = false;
                    // Drop the RHI: the ANativeWindow it borrowed is about to go invalid.
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
                    // Present one frame if we have a surface. The real path renders the
                    // interpolated snapshot here (invariant #4).
                    if let Some(rhi) = rhi.as_mut() {
                        if rhi.begin_frame() {
                            // TODO(phase1-step6): renderer.draw() against the latest snapshot.
                            rhi.end_frame();
                        }
                    }
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
                    // TODO(phase2): persist a resume snapshot here.
                }
                MainEvent::LowMemory => warn!("MainEvent::LowMemory"),
                _ => {}
            },
            // android-activity surfaces non-Main events (config changes, etc.) here.
            _ => {}
        });

        // Drain native input into the shared InputFrame each iteration. A real loop calls
        // this once per tick; the produced frame feeds the core's intent vocabulary.
        let _frame: InputFrame = input.poll();

        // Scaffold proof-of-life: while the surface is up, render (clear-present) every
        // iteration so the device shows a live frame regardless of RedrawNeeded cadence —
        // this confirms the wgpu/Vulkan round-trip on real hardware. The real fixed-tick
        // sim+render loop (mirroring app/src/main.rs) is wired in here in Phase 2.
        if window.surface_up {
            if let Some(rhi) = rhi.as_mut() {
                if rhi.begin_frame() {
                    rhi.end_frame();
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
}

impl AndroidInput {
    pub fn new(app: AndroidApp) -> Self {
        AndroidInput {
            app,
            frame: InputFrame::default(),
        }
    }

    /// Translate one motion (touch) event into the running InputFrame.
    fn apply_motion(frame: &mut InputFrame, motion: &MotionEvent) {
        // NOTE: android-activity 0.6 exposes pointers via `motion.pointers()`; we take the
        // primary pointer for the command-layer tap. Verify the exact pointer accessor on
        // the real toolchain — the crate has churned this surface across 0.5/0.6.
        let action = motion.action();
        if let Some(p) = motion.pointers().next() {
            frame.pointer = Some((p.x(), p.y()));
        }
        match action {
            MotionAction::Down | MotionAction::PointerDown => frame.pointer_down = true,
            MotionAction::Up | MotionAction::PointerUp | MotionAction::Cancel => {
                frame.pointer_down = false;
            }
            // Move keeps the current down-state; pointer position already updated above.
            _ => {}
        }
        // TODO(phase1-step6): on-screen virtual sticks -> move_axis / look_axis while
        //   embodied; gyro (via the ndk Sensor API) -> look_axis. Embody/surface gestures
        //   (e.g. long-press / dedicated button) -> embody_pressed / surface_pressed.
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

        // Drain the current native input batch. android-activity 0.6 hands input via an
        // iterator obtained from the app; we fold each event into `self.frame`.
        // NOTE: the exact call is `app.input_events_iter()` in 0.6 (returns a Result of an
        // iterator yielding `InputEvent`s). Confirm against the pinned crate on real
        // toolchain — older 0.5 used the `input_events(|e| ...)` closure form.
        if let Ok(mut iter) = self.app.input_events_iter() {
            // `next(&mut self, callback)`-style draining: process until the iterator is
            // exhausted, returning Handled so android-activity advances the queue.
            let frame = &mut self.frame;
            while iter.next(|event| {
                match event {
                    InputEvent::MotionEvent(motion) => Self::apply_motion(frame, motion),
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

/// [`Rhi`] backed by wgpu, which selects the **Vulkan** backend on Android automatically
/// (platforms.md §3). The surface is created from the `ANativeWindow` handed to us on
/// `InitWindow`; it must be recreated whenever the window is recreated (resume).
///
/// Holds the wgpu device/queue/surface and the current config so `resize` can reconfigure
/// the swapchain. `begin_frame` acquires the next surface texture (returning `false` if the
/// surface is lost so the caller skips and we recreate next frame); `end_frame` presents.
pub struct AndroidRhi {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The frame currently in flight between begin_frame/end_frame.
    frame: Option<wgpu::SurfaceTexture>,
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
            return Err(format!("surface.configure ({format:?} {width}x{height}): {err}"));
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
            frame: None,
        })
    }
}

impl Rhi for AndroidRhi {
    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn begin_frame(&mut self) -> bool {
        // wgpu 29 returns a `CurrentSurfaceTexture` enum (not a `Result`): `Suboptimal`
        // still yields a usable frame; `Lost`/`Outdated` mean reconfigure and skip.
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.frame = Some(frame);
                true
            }
            // Surface lost/outdated (common on resume): reconfigure and skip this frame.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                false
            }
            other => {
                warn!("get_current_texture: {other:?}");
                false
            }
        }
    }

    fn end_frame(&mut self) {
        if let Some(frame) = self.frame.take() {
            // TODO(phase1-step6): encode the shared `gonedark_render::Renderer` against
            //   `frame.texture` here (the desktop host already does this). For now we clear
            //   to a recognizable colour so a successful surface round-trip is VISIBLE on the
            //   device — an uncleared swapchain image is undefined (reads as black/garbage).
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("gonedark-android-clear"),
                    });
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("gonedark-android-clear-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            // "Going dark" indigo — distinct from black so it's unambiguous.
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.10,
                                g: 0.12,
                                b: 0.28,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            self.queue.submit(std::iter::once(encoder.finish()));
            frame.present();
        }
    }
}

// ---------------------------------------------------------------------------------------
// Audio + Storage — minimal honest stubs (Phase 1 audio is out of scope).
// ---------------------------------------------------------------------------------------

/// [`Audio`] stub. Real impl backends to **AAudio** (platforms.md §2) for low-latency
/// one-shots; the strategic→embodied "world goes dark" mix is engine-side and identical
/// everywhere (game-design.md §6).
#[derive(Default)]
pub struct AndroidAudio;

impl Audio for AndroidAudio {
    fn play_oneshot(&mut self, sound_id: u32) {
        // TODO(phase2): open an AAudio stream and mix one-shots. No-op for Phase 1.
        let _ = sound_id;
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
