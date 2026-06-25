//! Platform Abstraction Layer — trait definitions ONLY (invariant #2, D9).
//!
//! Concrete backends (`pal-desktop`, `pal-android`, later `pal-ios`) implement these;
//! `core` never sees them. Keep this seam *thin* — only what genuinely differs per
//! platform crosses it. Floats are fine here: this is the platform side, not the sim.
//!
//! [`mix`] is the one piece of *shared* logic in this crate: the per-voice audio render math
//! (pan/gain/muffle/sum) every backend mixes through — pure, float-only, host-testable, and
//! pulling no platform deps (see its module docs).

pub mod mix;

/// Monotonic clock for the run loop's frame timing.
pub trait Clock {
    /// Monotonic seconds since some fixed epoch.
    fn now_secs(&self) -> f64;
}

/// Per-frame input, already translated from the platform's native scheme into the
/// engine's intent vocabulary. Each backend maps touch+gyro / mouse+kbd / gamepad onto
/// this SAME struct, so the core stays input-agnostic (platforms.md §5).
#[derive(Clone, Debug, Default)]
pub struct InputFrame {
    /// Pointer position in window pixels, if any (command-layer tap/click).
    pub pointer: Option<(f32, f32)>,
    pub pointer_down: bool,
    /// Edge-triggered pointer-release (a tap/drag completed this frame). Touch UI uses this to
    /// close a drag-select rectangle or commit a tap. Latched + cleared like the other edges.
    pub pointer_up: bool,
    /// Edge-triggered embody / surface intents.
    pub embody_pressed: bool,
    pub surface_pressed: bool,
    /// "Open the order/stance context" input — a HELD/level signal: `true` for as long as the
    /// player holds the gesture (touch: a long-press held down; desktop: the F key held), NOT a
    /// one-shot edge. The command-UI layer opens the radial menu while it is held (a preview that
    /// emits no commands) and *commits* when a [`command_slot`](Self::command_slot) is chosen with
    /// it held; on release the menu closes. Held semantics are load-bearing — an edge would flash
    /// the menu for a single frame. See `engine::command_ui::radial_intent`.
    pub long_press: bool,
    /// A discrete order/stance vocabulary slot chosen from the on-screen command UI this frame,
    /// if any (touch: a radial/toolbar button; desktop: number keys). The command-UI layer maps
    /// the slot → a [`Command`](../../core) for the current selection. Kept as an opaque small
    /// integer so the PAL backend needn't know the vocabulary.
    pub command_slot: Option<u8>,
    /// Edge-triggered "command the current selection here" intent — the **right-click** in the
    /// classic-RTS desktop scheme (D42): a one-shot signal that the player issued the default order
    /// at [`pointer`](Self::pointer). The command-UI layer resolves it to a `Move` (empty ground) or
    /// `AttackMove` (on an enemy) across the whole selection. Distinct from
    /// [`pointer_down`](Self::pointer_down) (the left-click *selection* gesture) so selecting and
    /// commanding never collide on one button. Ignored while embodied. On touch (no right button)
    /// the per-platform scheme maps its own gesture here (Q4); the desktop backend latches it from
    /// the right mouse button.
    pub command_click: bool,
    /// **Single-pointer "tap commands" mode** (the touch scheme, D43). When set, a primary tap that
    /// lands *off* any friendly unit while a selection is active issues the default order
    /// (Move / Attack) to the selection — and **keeps** the selection — instead of deselecting.
    /// Tapping a friendly unit still *selects* it; a drag still band-selects. This is how a
    /// one-button touchscreen expresses what desktop splits across left-click (select) and
    /// right-click ([`command_click`](Self::command_click), command): there is no second button, so
    /// the engine resolves select-vs-command by *what was tapped*. Touch backends set this every
    /// frame (it is a mode, not an edge); desktop leaves it `false` (it has the dedicated
    /// right-click). Ignored while embodied.
    pub command_tap: bool,
    /// Command-view **build-palette** slot armed this frame, if any — a one-shot edge (like
    /// [`command_slot`](Self::command_slot)). The chosen structure is **placed at**
    /// [`pointer`](Self::pointer) (the cursor's ground point), so a build needs *both* this slot and
    /// a pointer. The engine's build seam (`engine::build_ui::build_commands`) maps it → a
    /// `Command::Build`, quantizing the placement point to `Fixed` at the boundary (invariant #1).
    /// Command view only (ignored while embodied). Backends set it per-platform: desktop latches a
    /// palette key; touch will latch an on-screen palette button (deferred with the rest of the
    /// on-screen command UI, like [`command_slot`](Self::command_slot)).
    pub building_slot: Option<u8>,
    /// Command-view **troop-training** slot armed this frame, if any — a one-shot edge. Names the
    /// unit archetype to queue at the player's active camp (the engine resolves *which* camp); the
    /// train seam (`engine::train_ui::train_commands`) maps it → a `Command::QueueProduction`.
    /// Command view only. No pointer needed — production targets the active camp, not a tapped point.
    pub train_slot: Option<u8>,
    /// Command-view **upgrade** intent — a one-shot edge: upgrade the player's active camp one tier.
    /// The upgrade seam (`engine::upgrade_ui::upgrade_commands`) maps it → a `Command::Upgrade`.
    /// Command view only; carries no pointer or slot (there is one upgrade action per camp today).
    pub upgrade_pressed: bool,
    /// Embodied locomotion + look axes (left stick / right stick or WASD + mouse).
    pub move_axis: (f32, f32),
    pub look_axis: (f32, f32),
    pub fire: bool,
}

/// A platform window / surface + lifecycle.
pub trait Window {
    fn size(&self) -> (u32, u32);
    fn should_close(&self) -> bool;
    /// Pump native events; returns false when the app should exit.
    fn pump(&mut self) -> bool;
}

/// Per-platform input source.
pub trait Input {
    fn poll(&mut self) -> InputFrame;
}

/// Render Hardware Interface — the GPU seam. Concrete impls wrap wgpu (→ Vulkan/D3D12/
/// Metal per device). The renderer talks to this, never to a specific GPU API.
pub trait Rhi {
    /// (Re)create the swapchain for a new surface size.
    fn resize(&mut self, width: u32, height: u32);
    /// Begin a frame; false means skip (e.g. surface lost — recreate next frame).
    fn begin_frame(&mut self) -> bool;
    /// Present the frame.
    fn end_frame(&mut self);
}

/// A distinct game sound. The embodied audio mix (game-design §6, invariant #6) is built from
/// these: strategic sound bleeding into the FPS view is the *primary* directional-awareness
/// system while the map is dark. Plain `repr` enum so the mix layer (engine) and the backends
/// (`pal-*`) share one vocabulary without either pulling a dependency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SoundId {
    /// A weapon firing somewhere on the field.
    Gunfire,
    /// One of your units died.
    UnitDown,
    /// A building of yours is being hit.
    BaseHit,
    /// A control point changed hands.
    Capture,
    /// A queued unit finished production.
    ProductionReady,
}

/// One positioned sound for this frame's mix. Floats are fine here (platform side, not the sim):
/// `azimuth`/`gain` are derived in the engine's *presentation* path from the deterministic event
/// stream + the listener pose — they never feed back into `core` (invariant #1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AudioCue {
    pub sound: SoundId,
    /// Bearing to the source relative to the listener's facing, radians. 0 = dead ahead,
    /// positive = to the right. Lets a backend pan the cue so direction reads by ear.
    pub azimuth: f32,
    /// Linear gain in `[0, 1]` after distance attenuation.
    pub gain: f32,
    /// When set, this is *strategic* sound leaking into the embodied mix — a backend should
    /// low-pass / duck it so it reads as "distant / off-map", not as a local event.
    pub muffled: bool,
}

/// Low-latency audio backend (AAudio/CoreAudio/PipeWire). `play_oneshot` is the simple fire-
/// and-forget path; `submit_mix` takes the per-frame positioned mix the embodied view needs
/// (the strategic-sound-into-FPS bleed, invariant #6). Output fidelity is per-backend; the
/// *mix* (which sounds, where, how loud) is computed once, platform-free, in `engine::audio`.
pub trait Audio {
    fn play_oneshot(&mut self, sound_id: u32);
    /// Submit this frame's positioned cues. Called every presented frame (often empty). A
    /// backend renders/pans/ducks them; an empty slice means silence this frame.
    fn submit_mix(&mut self, cues: &[AudioCue]);
}

/// Coarse thermal pressure reported by the platform (Phase 4 WS-C). Mirrors the shape of
/// Android's `PowerManager.getCurrentThermalStatus()` / iOS `ProcessInfo.thermalState` buckets,
/// collapsed to four levels the render-cost backoff policy reasons about. This is a **platform
/// signal**, so it lives on the PAL boundary and NEVER in `core` (invariant #2): the sim is
/// thermally blind by design — heat backs off *rendering* (cap FPS, lower the dyn-res floor), never
/// the deterministic 60 Hz tick (invariant #1/#4). The render-side policy that turns a state into a
/// backoff is `gonedark_render::tiers::thermal_backoff`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum ThermalState {
    /// No thermal pressure — full render freedom.
    #[default]
    Nominal,
    /// Mild warming — trim obvious waste (don't render faster than the sim ticks).
    Fair,
    /// Sustained heat — shed render cost (lower FPS cap, tighter dyn-res floor).
    Serious,
    /// Throttling imminent/active — survival: keep the game running and cool over pretty.
    Critical,
}

/// Battery / power-source signal reported by the platform (Phase 4 WS-C). Like [`ThermalState`]
/// this is a PAL-only platform signal (invariant #2) the *rendering* path may consult to bias the
/// quality tier / FPS cap (e.g. cap harder on a low, unplugged battery to extend playtime). It is
/// never a sim input. Kept deliberately coarse; `charge` is an optional `[0,1]` hint (`None` when
/// the platform doesn't report it).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PowerState {
    /// True when running on external power (charging / plugged in).
    pub on_external_power: bool,
    /// Battery charge fraction in `[0,1]`, if the platform reports it.
    pub charge: Option<f32>,
}

/// Platform thermal/power sensor seam (Phase 4 WS-C). A thin PAL query so the render-cost tuning
/// (FPS cap + dyn-res floor backoff) can react to heat/battery WITHOUT any platform/sensor code
/// leaking into `core` (invariant #2). Backends implement it from the OS:
///  - **Android** (OWED): `PowerManager.getThermalStatus()` + `BatteryManager`, read over JNI in
///    `pal-android` — this is where the on-device numbers that may reopen D21 dual-rate come from.
///  - **Desktop** (`pal-desktop`): no real thermal sensor on the dev workstation, so the backend
///    ships a clearly-marked synthetic/stub source (defaults to [`ThermalState::Nominal`], with a
///    test hook to drive the other states through the render backoff policy).
///
/// The sim never sees this trait — only the render/engine tuning path does, the same way only the
/// renderer touches `Rhi`.
pub trait ThermalSensor {
    /// The current thermal pressure bucket. Cheap to poll (expected once per frame / second).
    fn thermal_state(&self) -> ThermalState;
    /// The current power/battery state. Cheap to poll. Defaults to "on external power, charge
    /// unknown" so a backend with no battery (desktop) needn't override it.
    fn power_state(&self) -> PowerState {
        PowerState {
            on_external_power: true,
            charge: None,
        }
    }
}

/// Persistent storage / VFS seam (settings, replays).
pub trait Storage {
    fn read(&self, key: &str) -> Option<Vec<u8>>;
    fn write(&mut self, key: &str, bytes: &[u8]);
}

/// Network transport seam — opaque byte frames in, opaque byte frames out (D27). This is the
/// netcode's *what*, not its *how*: a frame is just `&[u8]`/`Vec<u8>`, and this trait names **no**
/// socket, UDP, QUIC, or relay type. Concrete backends (a loopback/in-process double for dev in
/// `pal-desktop`; real sockets, matchmaking, and relay in `server`) implement it; the abstract
/// trait stays protocol-free exactly as [`Audio`] stays audio-API-free.
///
/// The load-bearing boundary rule (D27 bullet 3): **the transport never understands a `Command`,
/// and `core` never understands a socket.** `core::lockstep` is sans-I/O — it *produces* outbound
/// frames and *consumes* inbound ones — and the host drives a `&mut dyn Transport` to move those
/// bytes between them. So this trait is deliberately object-safe (`&mut dyn Transport`) and deals
/// only in opaque frames: the lockstep loop assembles/parses the wire format in `core`, hands the
/// transport sealed bytes, and the transport never inspects them. Frames are delivered whole and
/// in order per direction; the transport neither splits nor merges them.
pub trait Transport {
    /// Hand one opaque outbound frame to the transport for delivery to the peer(s). The bytes are
    /// already the complete on-wire frame `core::lockstep` produced; the transport ships them
    /// verbatim and does not interpret them.
    fn send(&mut self, frame: &[u8]);

    /// Drain every inbound frame received since the last poll, in arrival order. Each `Vec<u8>` is
    /// one whole frame the host feeds back into `core::lockstep`. Returns empty when nothing has
    /// arrived — polling is cheap and expected every tick.
    fn poll(&mut self) -> Vec<Vec<u8>>;
}
