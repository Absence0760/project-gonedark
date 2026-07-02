//! The embodied alert HUD (invariant #6, game-design §6) — the *only* visual thread back to
//! command while the map is dark: directional pings ("taking fire to the south-east"), never a
//! map reveal. It draws ON TOP of the already-rendered embodied frame (a second pass that LOADs,
//! does not clear), so it is its own tiny screen-space pipeline + shader (`hud.wgsl`), kept
//! separate from the unit pipeline so the two never contend for the same shader/source.
//!
//! Data comes from `core::alerts::AlertChannel` (a presentation derivation, never sim state).
//! The HUD places each recent [`Alert`](gonedark_core::alerts::Alert) by the bearing of its
//! `pos` relative to the avatar's `yaw`, fading older ones out by tick age. Float boundary.
//!
//! ## Bearing → screen placement
//! Markers ride a ring near the screen edge. Bearing 0 (the alert lies straight ahead along
//! the avatar's facing) places the marker at top-center; a positive azimuth (the alert is to
//! the avatar's right) swings it clockwise toward the right edge, a negative one to the left.
//! An alert directly behind the avatar lands at bottom-center. The pure placement + fade math
//! lives in [`marker_for`] so it is unit-testable without a GPU.

use crate::text::Anchor;
use gonedark_core::alerts::{Alert, AlertChannel, AlertKind};
use wgpu::util::DeviceExt;

/// How many ticks an alert stays on the HUD before it has fully faded out. At 60 Hz this is a
/// ~2 s decay — long enough to read a direction, short enough that the thread stays *thin*.
/// Public so the host's presentation-side echo buffers (e.g. the accessibility visual-sound cues in
/// `engine`) can prune on the same window the HUD fades on.
pub const FADE_TICKS: u64 = 120;

/// The lowest alpha a still-live marker holds at (just before the hard cutoff at [`FADE_TICKS`]).
/// A linear ramp decays into <0.1 alpha that is invisible over a lit frame; flooring at a legible
/// value keeps the *whole* window readable, then the hard `None` at [`FADE_TICKS`] removes it.
const FADE_FLOOR: f32 = 0.35;

/// Half-size of a marker quad in NDC (markers are small screen-space chevrons/dots). `pub(crate)`
/// so the tank HUD's collision cross-check ([`crate::tank_hud`]) can size the alert-ring band it
/// must keep its turret chevron clear of (M1).
pub(crate) const MARKER_HALF_SIZE: f32 = 0.045;

/// Radius (in NDC) of the ring the markers sit on — near, but inside, the screen edge. `pub(crate)`
/// so the tank HUD's collision cross-check ([`crate::tank_hud`]) can locate the top-center alert
/// marker band its turret chevron must not overlap (M1).
pub(crate) const RING_RADIUS: f32 = 0.82;

/// Initial GPU capacity (in markers) for the instance buffer.
const INITIAL_CAP: usize = 16;

/// How many ticks the embodied hitmarker flash stays up after a connecting shot, fading linearly
/// to nothing. At 60 Hz this is a ~0.17 s snap — long enough to register "I hit him", short enough
/// to feel crisp and not smear across sustained fire (WS-4 / roadmap game-feel polish).
pub const HITMARKER_TICKS: u64 = 10;

/// Half-size (NDC) of the centered hitmarker quad — a small crosshair flash at screen center.
const HITMARKER_HALF_SIZE: f32 = 0.085;

/// The hitmarker is a crisp bright white "X" at screen center. White is deliberately distinct from
/// every other embodied-frame element: the alert-marker palette (warm red/orange, teal, pale grey)
/// rides the screen *edge* ring; the muzzle flash is a *warm* yellow (low blue); the FPS world is a
/// muted, low-saturation blue-grey. A high-blue near-white pixel at center reads only as the
/// hitmarker — which is exactly what the viz pixel-assert keys on.
const HITMARKER_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// The hitmarker glyph id matched by `glyph_coverage` in `hud.wgsl` (4 = centered "X").
const SHAPE_HITMARKER: f32 = 4.0;

/// The dot glyph id (`hud.wgsl` shape 0) — reused for the hip-fire crosshair ticks (WS-A).
const SHAPE_DOT: f32 = 0.0;

/// The hollow-ring glyph id (`hud.wgsl` shape 3). Public so the host can build accessibility
/// visual-sound-cue markers (the "distant capture" bleed echo) with the same glyph the alert HUD
/// uses for a place you no longer hold.
pub const SHAPE_RING: f32 = 3.0;

/// The plus/cross glyph id (`hud.wgsl` shape 5) — a "reinforcement ready" mark. Public so the host
/// can build the accessibility visual production-ready cue (the audio `ProductionReady` bell has no
/// alert-HUD equivalent; this is its visual parity for hard-of-hearing players — invariant #6).
pub const SHAPE_PLUS: f32 = 5.0;

/// Resting half-gap (NDC-y) from screen-center to each crosshair arm tick at zero recoil — a tight,
/// readable reticle. The recoil **bloom** ([`gonedark_engine::recoil::crosshair_bloom`]) is added to
/// this so the arms spread under fire and pull back as the gun settles (WS-A, CP-2 game-feel bar).
const CROSSHAIR_GAP: f32 = 0.030;

/// Half-size (NDC) of each small crosshair tick (the four arm dots + the center pip).
const CROSSHAIR_DOT_HALF: f32 = 0.011;

/// Crosshair color — a pale green-white that stays legible over the muted blue-grey FPS world
/// without reading as the warm muzzle flash or the red/orange alert ring (invariant #6 legibility).
const CROSSHAIR_COLOR: [f32; 3] = [0.86, 0.96, 0.90];

/// Crosshair opacity (it is constant chrome, not a fading flash).
const CROSSHAIR_ALPHA: f32 = 0.82;

/// The RGB color for an alert kind. Color is only ONE of the two cues — [`shape_for`] carries a
/// redundant shape so the kinds stay distinct for CVD players (invariant #6: the thread back must
/// stay legible). The palette is spaced along lightness *and* the blue-yellow axis, not just warm
/// hue, so no pair collapses to the same muddy yellow-brown under deuteranopia/protanopia.
fn alert_color(kind: AlertKind) -> [f32; 3] {
    match kind {
        AlertKind::BaseUnderAttack => [1.0, 0.12, 0.12], // hot, dark urgent red
        AlertKind::TakingFire => [1.0, 0.62, 0.15],      // high-value orange (lighter than red)
        AlertKind::TerritoryLost => [0.15, 0.52, 0.58],  // darker desaturated teal (off warm axis)
        AlertKind::UnitLost => [0.88, 0.88, 0.92],       // pale grey (a death; brightest)
    }
}

/// A redundant, non-color shape cue per alert kind, threaded to the shader as an instance attr.
/// Color alone fails ~8% of CVD players and over same-hue frame regions; the shape glyph keeps the
/// four kinds distinguishable while dark (invariant #6). The id is matched by `fs_main` in
/// `hud.wgsl`: 0 = filled dot, 1 = chevron, 2 = triangle, 3 = hollow ring.
fn shape_for(kind: AlertKind) -> f32 {
    match kind {
        AlertKind::BaseUnderAttack => 2.0, // triangle — the loudest, most urgent glyph
        AlertKind::TakingFire => 1.0,      // chevron — a directional "incoming"
        AlertKind::TerritoryLost => 3.0,   // hollow ring — a place you no longer hold
        AlertKind::UnitLost => 0.0,        // dot — a single quiet loss
    }
}

/// One screen-space directional marker, ready to upload. `repr(C)` + `Pod` so it streams
/// straight into the per-instance vertex buffer; the field order/offsets MUST match the
/// instance attribute locations in `hud.wgsl` and the `vertex_attr_array` in [`HudRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HudMarker {
    /// Marker center in normalized device coordinates ([-1,1], +y up).
    pub ndc_x: f32,
    pub ndc_y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Fade alpha in `[0,1]`.
    pub alpha: f32,
    /// Half-size of the marker quad in NDC.
    pub half_size: f32,
    /// Shape glyph id (redundant non-color cue): 0 dot, 1 chevron, 2 triangle, 3 ring.
    pub shape: f32,
}

/// Compute the screen-space marker for one `alert` from the avatar's pose, or `None` if the
/// alert has faded out (age beyond [`FADE_TICKS`], or its tick is in the future).
///
/// Bearing is the signed angle from the avatar's facing (`yaw`) to the alert direction:
/// `0` → straight ahead (top-center), `+` → to the right, `-` → to the left, `±π` → behind
/// (bottom-center). Pure float math (presentation boundary), so it is unit-testable.
pub fn marker_for(
    alert: &Alert,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
) -> Option<HudMarker> {
    place_marker(
        (
            crate::fixed_to_f32(alert.pos.x),
            crate::fixed_to_f32(alert.pos.y),
        ),
        alert.tick,
        avatar_world,
        yaw,
        tick,
        alert_color(alert.kind),
        shape_for(alert.kind),
    )
}

/// Shared placement + age-fade math for one edge-ring directional marker at world point `pos_world`,
/// stamped on `event_tick`, given the avatar pose and current `tick`, with a caller-chosen `color`
/// and `shape` glyph. Returns `None` once faded ([`FADE_TICKS`]) or for a future-stamped event.
///
/// [`marker_for`] wraps this for [`Alert`]s; the host's accessibility visual-sound cues (`engine`)
/// call it directly to place production-ready / distant-capture echoes on the SAME ring with the same
/// bearing + fade behaviour, so the two threads read consistently. Pure float math (presentation
/// boundary), unit-testable without a GPU. Reveals only a direction (invariant #6).
pub fn place_marker(
    pos_world: (f32, f32),
    event_tick: u64,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
    color: [f32; 3],
    shape: f32,
) -> Option<HudMarker> {
    // Age-based fade. A future-stamped event (tick < event_tick) is treated as not-yet-live.
    if tick < event_tick {
        return None;
    }
    let age = tick - event_tick;
    if age >= FADE_TICKS {
        return None;
    }
    // Ease-out fade: reads full-bright immediately, then decays on an ease-out curve that holds a
    // legible [`FADE_FLOOR`] until the hard `None` at [`FADE_TICKS`] removes it. (A linear ramp
    // kept old and new pings near-equal for half their life then sank into invisible <0.1 alpha.)
    let t = age as f32 / FADE_TICKS as f32; // 0 at fresh → 1 at the cutoff
    let eased = (1.0 - t) * (1.0 - t); // ease-out: steep early read, gentle tail
    let alpha = FADE_FLOOR + (1.0 - FADE_FLOOR) * eased;

    // Direction from the avatar to the event, in world space.
    let (ax, ay) = avatar_world;
    let dx = pos_world.0 - ax;
    let dy = pos_world.1 - ay;

    // World bearing of the event, then relative to the avatar's facing. We use atan2 so a zero
    // vector (event on top of the avatar) still yields a stable bearing (0 → straight ahead).
    let world_bearing = dy.atan2(dx);
    // Signed azimuth in (-π, π], 0 = dead ahead, + = to the right. atan2 grows
    // counter-clockwise, so "to the right" (clockwise from facing) is `yaw - world_bearing`.
    let mut azimuth = yaw - world_bearing;
    let two_pi = std::f32::consts::TAU;
    azimuth = ((azimuth + std::f32::consts::PI).rem_euclid(two_pi)) - std::f32::consts::PI;

    // Map azimuth → a point on the ring. azimuth 0 must land at top-center (ndc = (0, +R)),
    // positive azimuth must swing to the right (ndc_x > 0). With +y up that is:
    //   ndc_x =  R * sin(azimuth)
    //   ndc_y =  R * cos(azimuth)
    let ndc_x = RING_RADIUS * azimuth.sin();
    let ndc_y = RING_RADIUS * azimuth.cos();

    let [r, g, b] = color;
    Some(HudMarker {
        ndc_x,
        ndc_y,
        r,
        g,
        b,
        alpha,
        half_size: MARKER_HALF_SIZE,
        shape,
    })
}

/// A short, non-color abbreviation for an alert kind — the **colorblind (CVD) cue** (invariant #6).
/// The shape glyph ([`shape_for`]) + luminance-spread palette ([`alert_color`]) already make the four
/// kinds distinguishable without hue, but a hue-blind player under a same-hue frame region can still
/// be unsure *which* kind a glyph is; a two-to-four-letter label removes all ambiguity. Kept terse so
/// it stays legible at marker size and never crowds the thin thread back.
pub fn alert_label(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::TakingFire => "FIRE",
        AlertKind::UnitLost => "LOST",
        AlertKind::BaseUnderAttack => "BASE",
        AlertKind::TerritoryLost => "TERR",
    }
}

/// NDC gap from a marker's center down to the top of its CVD text label, so the label rides just
/// beneath the glyph without overlapping it.
const LABEL_DROP: f32 = MARKER_HALF_SIZE + 0.012;

/// One placed CVD text label: the abbrev, its top-center NDC anchor, color, and fade alpha. Built by
/// [`alert_labels`] so the (pure) placement is testable without a `TextRenderer`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudLabel {
    pub text: &'static str,
    pub ndc_x: f32,
    pub ndc_y: f32,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Build the CVD text labels for every live alert — one abbrev ([`alert_label`]) placed just below
/// its ring marker, sharing the marker's color + fade alpha. Empty when nothing is live. Pure
/// (presentation boundary), so the placement is unit-testable; the host draws them through the shared
/// `TextRenderer` only when the "Colorblind cues" toggle is on.
pub fn alert_labels(
    alerts: &AlertChannel,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
) -> Vec<HudLabel> {
    alerts
        .recent
        .iter()
        .filter_map(|a| {
            let m = marker_for(a, avatar_world, yaw, tick)?;
            Some(HudLabel {
                text: alert_label(a.kind),
                ndc_x: m.ndc_x,
                ndc_y: m.ndc_y - LABEL_DROP,
                color: [m.r, m.g, m.b],
                alpha: m.alpha,
            })
        })
        .collect()
}

/// Build the centered hitmarker for this frame from `last_hit_tick` (the tick the embodied avatar's
/// own shot last connected) and the current `tick`, or `None` when no hit is live (never hit, a
/// future-stamped tick, or aged past [`HITMARKER_TICKS`]).
///
/// The hitmarker is presentation feedback on the player's OWN action — it draws iff the avatar
/// itself dealt damage — so it reveals nothing about unseen enemies and stays inside invariant #6
/// (WS-4). A pure float fn (the presentation boundary) so it is unit-testable without a GPU. Placed
/// dead-center (NDC `(0, 0)`) so it never collides with the edge-ring alert markers.
pub fn hitmarker_marker(last_hit_tick: Option<u64>, tick: u64) -> Option<HudMarker> {
    let fired = last_hit_tick?;
    if tick < fired {
        return None; // future-stamped hit is not yet live
    }
    let age = tick - fired;
    if age >= HITMARKER_TICKS {
        return None;
    }
    // Linear fade: full-bright on the connecting frame, gone by HITMARKER_TICKS.
    let alpha = 1.0 - age as f32 / HITMARKER_TICKS as f32;
    let [r, g, b] = HITMARKER_COLOR;
    Some(HudMarker {
        ndc_x: 0.0,
        ndc_y: 0.0,
        r,
        g,
        b,
        alpha,
        half_size: HITMARKER_HALF_SIZE,
        shape: SHAPE_HITMARKER,
    })
}

/// Build the hip-fire **dynamic crosshair** — four arm ticks (up/down/left/right) plus a center pip —
/// for the current recoil `bloom` and viewport `aspect` (WS-A, CP-2 game-feel bar). The arms sit a
/// half-gap of `CROSSHAIR_GAP + bloom` from screen-center, so at rest (`bloom == 0`) the reticle is
/// tight and under fire it **spreads** outward and settles back as the recoil decays. The horizontal
/// gap is divided by `aspect` so the cross stays square on a wide window (the raw-NDC chrome footgun —
/// the alert markers ride a ring and dodge it, but a crosshair's symmetry needs the correction).
///
/// Pure float math (presentation boundary), so it is unit-testable without a GPU. Reveals nothing —
/// it is screen-center chrome with no world position (invariant #6).
pub fn crosshair_markers(bloom: f32, aspect: f32) -> Vec<HudMarker> {
    let aspect = if aspect.abs() < 1.0e-6 { 1.0 } else { aspect };
    let gap = (CROSSHAIR_GAP + bloom.max(0.0)).max(0.0);
    let gx = gap / aspect; // square the cross on a non-1:1 viewport
    let [r, g, b] = CROSSHAIR_COLOR;
    let tick = |ndc_x: f32, ndc_y: f32| HudMarker {
        ndc_x,
        ndc_y,
        r,
        g,
        b,
        alpha: CROSSHAIR_ALPHA,
        half_size: CROSSHAIR_DOT_HALF,
        shape: SHAPE_DOT,
    };
    vec![
        tick(0.0, 0.0),  // center pip
        tick(gx, 0.0),   // right arm
        tick(-gx, 0.0),  // left arm
        tick(0.0, gap),  // top arm
        tick(0.0, -gap), // bottom arm
    ]
}

// --- desktop "SURFACE" key reminder (M5) -------------------------------------------------------

/// Opacity of the Surface key hint — deliberately low so it reads as a quiet reminder, never
/// competing with the alert thread back or the reticle.
const SURFACE_HINT_ALPHA: f32 = 0.55;
/// Glyph cell height (NDC) of the Surface key hint — small chrome text.
const SURFACE_HINT_SIZE: f32 = 0.038;
/// The hint rides low, well clear of the top-center alert ring and the screen-center reticle.
const SURFACE_HINT_Y: f32 = -0.80;
/// A muted tint so the reminder recedes until the player needs it.
const SURFACE_HINT_COLOR: [f32; 3] = crate::theme::ASH;

/// One laid-out Surface key-reminder label for the text pass. Mirrors the label structs elsewhere
/// (`HudLabel`, `PromptLabel`) so its placement is unit-testable without a `TextRenderer`.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceHint {
    pub text: String,
    pub pos: [f32; 2],
    pub size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// The desktop-only "[key] SURFACE" reminder (M5): while embodied, keyboard players have no
/// on-screen Surface button (that is a touch affordance), so a low-opacity hint tells them which
/// key surfaces them back to command. Returns `None` on touch (the touch HUD already draws a
/// Surface button) or for an empty `label`, so it is **gated to embodied-AND-not-touch** — the
/// caller only invokes it while embodied, and this drops it on touch.
///
/// `label` is the fully-built hint string (e.g. `"[F] SURFACE"`); the host composes it from the
/// live keybind (that lookup lives in `engine`, not here). Pure screen-space chrome with no world
/// position (invariant #6) — it reveals nothing the avatar's own eyes don't. Pure + GPU-free →
/// unit-tested.
pub fn surface_reminder(label: &str, is_touch: bool) -> Option<SurfaceHint> {
    if is_touch || label.is_empty() {
        return None;
    }
    Some(SurfaceHint {
        text: label.to_string(),
        pos: [0.0, SURFACE_HINT_Y],
        size: SURFACE_HINT_SIZE,
        anchor: Anchor::TopCenter,
        color: SURFACE_HINT_COLOR,
        alpha: SURFACE_HINT_ALPHA,
    })
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-marker half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// The two triangles of a unit quad.
const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex {
        corner: [1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, 1.0],
    },
];

/// Screen-space directional-alert overlay for the embodied view. Owns its own pipeline +
/// buffers (a separate pipeline from the unit pass so the two never contend for a shader).
pub struct HudRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    /// Per-instance GPU buffer of [`HudMarker`]; reallocated only when it must grow.
    instance_buf: wgpu::Buffer,
    /// Capacity (in markers) currently allocated in `instance_buf`.
    instance_cap: usize,
}

impl HudRenderer {
    /// Build the HUD pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Uses alpha blending so markers composite over the already-rendered frame.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.hud_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hud.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.hud_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HudMarker>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=color(vec3), 3=alpha(f32), 4=half_size(f32), 5=shape(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x3,
                3 => Float32,
                4 => Float32,
                5 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.hud_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[quad_layout, instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.hud_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.hud_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<HudMarker>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        HudRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw this frame's directional alert markers on top of `view` (a LOAD pass — never clears).
    ///
    /// - `alerts`: the rolling alert channel (most recent last).
    /// - `avatar_world`: the listener/avatar position in world units.
    /// - `yaw`: avatar facing (radians) — markers are placed by bearing relative to this.
    /// - `_viewport`: surface size in pixels — unused (placement is pure NDC ring math); kept on
    ///   this signature only because the `lib.rs` boundary still threads it. [`marker_for`] no
    ///   longer takes it. (If aspect correction is ever wanted, re-add a real param there.)
    /// - `tick`: the current sim tick (to fade alerts by age).
    ///
    /// Builds the live marker set via [`marker_for`], uploads it, and records a single LOAD
    /// render pass so the markers composite over the embodied frame. No-op if nothing is live.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        alerts: &AlertChannel,
        avatar_world: (f32, f32),
        yaw: f32,
        _viewport: (u32, u32),
        tick: u64,
    ) {
        let markers: Vec<HudMarker> = alerts
            .recent
            .iter()
            .filter_map(|a| marker_for(a, avatar_world, yaw, tick))
            .collect();

        // No live markers → nothing to draw (and the frame must stay untouched).
        self.draw_markers(device, queue, view, &markers);
    }

    /// Draw the embodied hitmarker — the centered "X" flash confirming the player's OWN shot
    /// connected (WS-4). Builds the single live marker via [`hitmarker_marker`] from `last_hit_tick`
    /// and `tick`, then composites it as a LOAD pass over the embodied frame (same pipeline/shader as
    /// the alert markers, glyph 4). No-op when no hit is live, so it leaves the frame untouched.
    ///
    /// Invariant #6: this is feedback on the avatar's own action (the caller only stamps
    /// `last_hit_tick` when the avatar itself dealt damage), not intel about an unseen enemy — it
    /// reveals nothing the player could not already see.
    pub fn render_hitmarker(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        last_hit_tick: Option<u64>,
        tick: u64,
    ) {
        let Some(marker) = hitmarker_marker(last_hit_tick, tick) else {
            return;
        };
        self.draw_markers(device, queue, view, &[marker]);
    }

    /// Draw the hip-fire **dynamic crosshair** (WS-A) — the four arm ticks + center pip, spread by the
    /// recoil `bloom` — as a LOAD pass over the embodied frame (same pipeline/shader as the alert
    /// markers, glyph 0). `aspect` keeps the cross square on a wide viewport. The host calls this only
    /// while embodied in a unit that hip-fires (infantry); the tank shows its own gun-sight reticle
    /// instead. Screen-center chrome with no world position — reveals nothing (invariant #6).
    pub fn render_crosshair(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        bloom: f32,
        aspect: f32,
    ) {
        let markers = crosshair_markers(bloom, aspect);
        self.draw_markers(device, queue, view, &markers);
    }

    /// Draw a caller-built set of directional `markers` as a LOAD pass over the embodied frame (same
    /// pipeline/shader as the alert overlay). Used by the host's **accessibility visual-sound cues**
    /// (the hard-of-hearing production-ready / distant-capture echoes it builds via [`place_marker`]),
    /// so they composite on the same edge ring as the alert markers. No-op on an empty set.
    pub fn render_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        markers: &[HudMarker],
    ) {
        self.draw_markers(device, queue, view, markers);
    }

    /// Upload `markers` and composite them as one LOAD pass over `view` (never clears). Shared by
    /// the alert overlay ([`Self::render`]) and the hitmarker ([`Self::render_hitmarker`]) so both
    /// drive the one screen-space pipeline. No-op on an empty set so the frame stays untouched.
    fn draw_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        markers: &[HudMarker],
    ) {
        if markers.is_empty() {
            return;
        }

        if markers.len() > self.instance_cap {
            let new_cap = markers.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.hud_instance_vbo"),
                size: (new_cap * std::mem::size_of::<HudMarker>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(markers));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.hud_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.hud_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // LOAD, never Clear — this draws on top of the embodied frame.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..markers.len() as u32);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32`
    //! math and epsilon comparisons are fair game. `HudRenderer::new` needs a real `wgpu::Device`
    //! (no display in CI), so the pipeline path is untested; the testable placement/fade math is
    //! factored into [`marker_for`].

    use super::*;
    use gonedark_core::components::Vec2;
    use gonedark_core::fixed::Fixed;

    fn alert(kind: AlertKind, x: i32, y: i32, tick: u64) -> Alert {
        Alert {
            kind,
            pos: Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
            tick,
        }
    }

    // ---- bearing → placement ----

    #[test]
    fn dead_ahead_is_top_center() {
        // Avatar at origin facing +x (yaw = 0); alert straight ahead along +x.
        let a = alert(AlertKind::TakingFire, 10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).expect("fresh alert is live");
        // Top-center: small |ndc_x|, ndc_y near +RING_RADIUS (top of screen).
        assert!(m.ndc_x.abs() < 1e-3, "ndc_x={} should be ~0", m.ndc_x);
        assert!(m.ndc_y > 0.5, "ndc_y={} should be near top", m.ndc_y);
    }

    #[test]
    fn alert_to_the_right_has_positive_ndc_x() {
        // Avatar at origin facing +x; alert to the avatar's right is at -y (screen-right when
        // facing +x with +y "left/up" in world). Direction (0,-10) relative to yaw 0 → azimuth
        // -π/2 maps to... use a direction that is unambiguously to the right of facing:
        // facing +x, "right" in world for +y-up convention is -y.
        let a = alert(AlertKind::TakingFire, 0, -10, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x > 0.0, "ndc_x={} should be on the right", m.ndc_x);
    }

    #[test]
    fn alert_to_the_left_has_negative_ndc_x() {
        // Facing +x; alert at +y is to the avatar's left.
        let a = alert(AlertKind::TakingFire, 0, 10, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x < 0.0, "ndc_x={} should be on the left", m.ndc_x);
    }

    #[test]
    fn alert_behind_is_bottom_center() {
        // Facing +x; alert at -x is directly behind → azimuth ±π → bottom-center.
        let a = alert(AlertKind::TakingFire, -10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x.abs() < 1e-3, "ndc_x={} should be ~0", m.ndc_x);
        assert!(m.ndc_y < -0.5, "ndc_y={} should be near bottom", m.ndc_y);
    }

    #[test]
    fn yaw_rotates_placement() {
        // Alert along +x, but avatar now faces +x rotated by +π/2 (faces +y). The alert is now
        // to the avatar's right → ndc_x > 0.
        let a = alert(AlertKind::TakingFire, 10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), std::f32::consts::FRAC_PI_2, 0).unwrap();
        assert!(
            m.ndc_x > 0.0,
            "ndc_x={} should swing right under yaw",
            m.ndc_x
        );
    }

    // ---- fade by age ----

    #[test]
    fn fresh_alert_is_full_alpha() {
        let a = alert(AlertKind::UnitLost, 5, 5, 100);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 100).unwrap();
        assert!(
            (m.alpha - 1.0).abs() < 1e-4,
            "alpha={} should be ~1",
            m.alpha
        );
    }

    #[test]
    fn aging_alert_fades_toward_floor() {
        // The ease-out curve must decrease monotonically with age, but never below FADE_FLOOR
        // while the marker is still live — it stays legible right up to the hard cutoff.
        let a = alert(AlertKind::UnitLost, 5, 5, 0);
        let young = marker_for(&a, (0.0, 0.0), 0.0, 10).unwrap();
        let mid = marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS / 2).unwrap();
        let old = marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS - 1).unwrap();
        assert!(young.alpha > mid.alpha, "alpha should decrease with age");
        assert!(mid.alpha > old.alpha, "alpha should keep decreasing");
        // Floored: even the oldest live marker stays >= the legibility floor (not invisible).
        assert!(
            old.alpha >= FADE_FLOOR,
            "near-end alpha {} should hold the floor {}",
            old.alpha,
            FADE_FLOOR
        );
        // ...and never exceeds 1.0.
        assert!(young.alpha <= 1.0, "alpha must stay <= 1");
    }

    #[test]
    fn faded_out_alert_is_none() {
        let a = alert(AlertKind::TakingFire, 5, 5, 0);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS).is_none());
        assert!(marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS + 50).is_none());
    }

    #[test]
    fn future_alert_is_none() {
        // tick < alert.tick (not yet live) yields None rather than a negative age.
        let a = alert(AlertKind::TakingFire, 5, 5, 100);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, 50).is_none());
    }

    #[test]
    fn color_tracks_kind() {
        let p = (0.0, 0.0);
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        // Each kind's marker stamps exactly its `alert_color`.
        let cols: Vec<[f32; 3]> = kinds
            .iter()
            .map(|&k| {
                let m = marker_for(&alert(k, 1, 0, 0), p, 0.0, 0).unwrap();
                assert_eq!([m.r, m.g, m.b], alert_color(k), "color must track {:?}", k);
                [m.r, m.g, m.b]
            })
            .collect();
        // All four colors are pairwise distinct (no two kinds share an RGB).
        for i in 0..cols.len() {
            for j in (i + 1)..cols.len() {
                assert_ne!(
                    cols[i], cols[j],
                    "{:?} and {:?} must not share a color",
                    kinds[i], kinds[j]
                );
            }
        }
    }

    /// Relative luminance (Rec. 709) of an RGB triple — the channel a CVD viewer still reads.
    fn luminance([r, g, b]: [f32; 3]) -> f32 {
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    #[test]
    fn alert_palette_separates_by_luminance() {
        // The accessibility fix: every pair of kinds must differ in lightness, not just hue, so
        // they don't collapse under red-green CVD (invariant #6's thread back must stay legible).
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        for i in 0..kinds.len() {
            for j in (i + 1)..kinds.len() {
                let li = luminance(alert_color(kinds[i]));
                let lj = luminance(alert_color(kinds[j]));
                assert!(
                    (li - lj).abs() > 0.05,
                    "{:?} (L={:.3}) and {:?} (L={:.3}) need a luminance spread",
                    kinds[i],
                    li,
                    kinds[j],
                    lj
                );
            }
        }
    }

    #[test]
    fn shape_tracks_kind() {
        // The redundant non-color cue: each kind stamps a distinct shape glyph id, so the four
        // kinds stay distinguishable for CVD players / same-hue frame regions (invariant #6).
        let p = (0.0, 0.0);
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        let shapes: Vec<f32> = kinds
            .iter()
            .map(|&k| {
                let m = marker_for(&alert(k, 1, 0, 0), p, 0.0, 0).unwrap();
                assert_eq!(m.shape, shape_for(k), "shape must track {:?}", k);
                m.shape
            })
            .collect();
        // All four shape ids are pairwise distinct.
        for i in 0..shapes.len() {
            for j in (i + 1)..shapes.len() {
                assert_ne!(
                    shapes[i], shapes[j],
                    "{:?} and {:?} must not share a shape",
                    kinds[i], kinds[j]
                );
            }
        }
    }

    // ---- hitmarker (WS-4) ----

    #[test]
    fn no_hit_yet_is_no_marker() {
        assert!(hitmarker_marker(None, 0).is_none());
        assert!(hitmarker_marker(None, 500).is_none());
    }

    #[test]
    fn fresh_hit_is_centered_full_bright_x() {
        let m = hitmarker_marker(Some(100), 100).expect("a hit this tick is live");
        // Dead center so it never collides with the edge-ring alert markers.
        assert!(m.ndc_x.abs() < 1e-6 && m.ndc_y.abs() < 1e-6, "centered");
        assert!((m.alpha - 1.0).abs() < 1e-4, "alpha={} should be ~1", m.alpha);
        // The distinct hitmarker glyph + bright white color the viz pixel-assert keys on.
        assert_eq!(m.shape, SHAPE_HITMARKER);
        assert_eq!([m.r, m.g, m.b], HITMARKER_COLOR);
    }

    #[test]
    fn hitmarker_fades_monotonically_then_vanishes() {
        let young = hitmarker_marker(Some(0), 1).unwrap();
        let mid = hitmarker_marker(Some(0), HITMARKER_TICKS / 2).unwrap();
        let old = hitmarker_marker(Some(0), HITMARKER_TICKS - 1).unwrap();
        assert!(young.alpha > mid.alpha && mid.alpha > old.alpha, "fades with age");
        assert!(old.alpha > 0.0 && young.alpha <= 1.0, "stays in (0, 1]");
        // Aged past the window → gone (the frame is left untouched).
        assert!(hitmarker_marker(Some(0), HITMARKER_TICKS).is_none());
        assert!(hitmarker_marker(Some(0), HITMARKER_TICKS + 50).is_none());
    }

    #[test]
    fn future_stamped_hit_is_none() {
        // tick < last_hit_tick (clock not yet there) yields None rather than a negative age.
        assert!(hitmarker_marker(Some(100), 50).is_none());
    }

    // ---- dynamic crosshair (WS-A) ----

    #[test]
    fn crosshair_has_center_pip_plus_four_arms() {
        let m = crosshair_markers(0.0, 1.0);
        assert_eq!(m.len(), 5, "center + 4 arms");
        // The first is the center pip.
        assert!(m[0].ndc_x.abs() < 1e-9 && m[0].ndc_y.abs() < 1e-9, "center pip at origin");
        // Every tick is the dot glyph in the crosshair color.
        for t in &m {
            assert_eq!(t.shape, SHAPE_DOT);
            assert_eq!([t.r, t.g, t.b], CROSSHAIR_COLOR);
        }
        // At rest the arms sit at the resting gap.
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap();
        assert!((right.ndc_x - CROSSHAIR_GAP).abs() < 1e-6, "rest gap at aspect 1");
    }

    #[test]
    fn crosshair_blooms_outward_with_recoil() {
        let rest = crosshair_markers(0.0, 1.0);
        let fired = crosshair_markers(0.05, 1.0);
        let rest_right = rest.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let fired_right = fired.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let rest_top = rest.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        let fired_top = fired.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        assert!(fired_right > rest_right, "horizontal arms spread under fire");
        assert!(fired_top > rest_top, "vertical arms spread under fire");
        // The bloom adds exactly the recoil bloom to the gap (aspect 1).
        assert!((fired_top - (CROSSHAIR_GAP + 0.05)).abs() < 1e-6);
    }

    #[test]
    fn crosshair_stays_square_on_a_wide_window() {
        // aspect 16:9 → the horizontal gap is divided by aspect so the on-screen cross is square
        // (the same raw-NDC chrome footgun the alert ring dodges). Multiply the x gap back by aspect
        // and it matches the y gap.
        let aspect = 16.0 / 9.0;
        let m = crosshair_markers(0.02, aspect);
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let top = m.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        assert!((right * aspect - top).abs() < 1e-6, "x·aspect ({right}) matches y ({top})");
    }

    #[test]
    fn crosshair_negative_bloom_does_not_pull_arms_inside_rest() {
        // A stray negative recoil can never collapse the reticle past its resting gap.
        let m = crosshair_markers(-1.0, 1.0);
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        assert!((right - CROSSHAIR_GAP).abs() < 1e-6, "floors at the resting gap");
    }

    // ---- desktop SURFACE key reminder (M5) ----

    #[test]
    fn surface_reminder_is_desktop_only() {
        // Touch has an on-screen Surface button, so the key hint is suppressed there.
        assert!(
            surface_reminder("[F] SURFACE", true).is_none(),
            "touch already has a Surface button — no key hint"
        );
        // Nothing to say with no label.
        assert!(surface_reminder("", false).is_none(), "empty label → no hint");
        // Desktop (not touch) with a label gets the hint.
        let h = surface_reminder("[F] SURFACE", false).expect("desktop shows the key hint");
        assert_eq!(h.text, "[F] SURFACE", "carries the caller's keybind label verbatim");
        assert_eq!(h.anchor, Anchor::TopCenter);
    }

    #[test]
    fn surface_reminder_is_quiet_low_screen_space_chrome() {
        let h = surface_reminder("[F] SURFACE", false).unwrap();
        // Low opacity — a reminder, not chrome that competes with the alert thread.
        assert!(h.alpha > 0.0 && h.alpha < 0.7, "hint is low-opacity, got {}", h.alpha);
        // Screen-space bounded with no world position (invariant #6).
        assert!(h.pos[0].abs() <= 1.0 && h.pos[1].abs() <= 1.0, "hint is on-screen NDC");
        // Rides low, clear of the top-center alert ring (RING_RADIUS) and the screen-center reticle.
        assert!(h.pos[1] < 0.0, "hint sits in the lower screen, off the alert ring");
        assert!(h.pos[1] > -1.0 + h.size, "hint stays on-screen above the bottom edge");
    }

    // ---- CVD text labels + place_marker (accessibility) ----

    #[test]
    fn alert_label_is_distinct_and_short_per_kind() {
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        let labels: Vec<&str> = kinds.iter().map(|&k| alert_label(k)).collect();
        for l in &labels {
            assert!(!l.is_empty() && l.len() <= 4, "label {l:?} stays terse");
            assert!(
                l.chars().all(|c| c.is_ascii_uppercase()),
                "label {l:?} is drawable uppercase ASCII (in the font atlas)"
            );
        }
        // All four abbrevs are pairwise distinct — the whole point of the CVD cue.
        for i in 0..labels.len() {
            for j in (i + 1)..labels.len() {
                assert_ne!(labels[i], labels[j], "labels must be unique");
            }
        }
    }

    #[test]
    fn alert_labels_track_live_markers_and_sit_below_them() {
        // Two live alerts + one already faded → two labels, each carrying its kind's abbrev, color,
        // and fade alpha, placed just under its ring marker.
        let mut ch = AlertChannel::new();
        ch.recent.push(alert(AlertKind::TakingFire, 10, 0, 0)); // dead ahead
        ch.recent.push(alert(AlertKind::UnitLost, -10, 0, 0)); // behind
        ch.recent.push(alert(AlertKind::BaseUnderAttack, 5, 5, 0)); // faded out below
        let tick = 5;
        let labels = alert_labels(&ch, (0.0, 0.0), 0.0, tick);
        assert_eq!(labels.len(), 3, "all three are still live at tick 5");
        // Each label sits LABEL_DROP below its marker center and mirrors its color/alpha.
        for (a, l) in ch.recent.iter().zip(labels.iter()) {
            let m = marker_for(a, (0.0, 0.0), 0.0, tick).unwrap();
            assert_eq!(l.text, alert_label(a.kind));
            assert_eq!(l.color, [m.r, m.g, m.b]);
            assert!((l.alpha - m.alpha).abs() < 1e-6);
            assert!((l.ndc_x - m.ndc_x).abs() < 1e-6, "label shares the marker column");
            assert!(l.ndc_y < m.ndc_y, "label rides below the marker");
        }
        // A fully faded alert drops its label too.
        let faded = alert_labels(&ch, (0.0, 0.0), 0.0, FADE_TICKS + 1);
        assert!(faded.is_empty());
    }

    #[test]
    fn place_marker_matches_marker_for_for_an_alert() {
        // The shared seam must reproduce marker_for exactly for the same alert (they're one code path).
        let a = alert(AlertKind::BaseUnderAttack, 3, -7, 2);
        let via_alert = marker_for(&a, (1.0, 1.0), 0.4, 20).unwrap();
        let via_place = place_marker(
            (3.0, -7.0),
            2,
            (1.0, 1.0),
            0.4,
            20,
            alert_color(a.kind),
            shape_for(a.kind),
        )
        .unwrap();
        assert_eq!(via_alert, via_place);
    }

    #[test]
    fn place_marker_carries_caller_shape_and_color_and_fades() {
        // Host echoes pick their own glyph/color (e.g. the reinforcement plus) — place_marker must
        // honor them, fade by age, and vanish past the window / for a future stamp.
        let color = [0.30, 0.85, 0.45];
        let m = place_marker((10.0, 0.0), 0, (0.0, 0.0), 0.0, 0, color, SHAPE_PLUS).unwrap();
        assert_eq!([m.r, m.g, m.b], color);
        assert_eq!(m.shape, SHAPE_PLUS);
        assert!((m.alpha - 1.0).abs() < 1e-4, "fresh echo is full alpha");
        assert!(place_marker((10.0, 0.0), 0, (0.0, 0.0), 0.0, FADE_TICKS, color, SHAPE_PLUS).is_none());
        assert!(place_marker((10.0, 0.0), 100, (0.0, 0.0), 0.0, 50, color, SHAPE_PLUS).is_none());
    }

    #[test]
    fn marker_position_encodes_only_bearing_not_distance() {
        // Fairness guard (invariant #6) that holds under EVERY accessibility cue mode: the alert is a
        // DIRECTIONAL ping, never a position. Two events on the same bearing from the avatar but at
        // very different ranges must land at the SAME screen point — the marker leaks a direction, not
        // a distance (which would be strategic intel while the map is dark). The CVD labels
        // (`alert_labels`) and the host's visual-sound echoes both go through this same `place_marker`
        // ring math, so proving it here proves it for every cue mode.
        let near = alert(AlertKind::TakingFire, 3, 0, 0); // dead ahead, close
        let far = alert(AlertKind::TakingFire, 300, 0, 0); // dead ahead, far
        let mn = marker_for(&near, (0.0, 0.0), 0.0, 0).unwrap();
        let mf = marker_for(&far, (0.0, 0.0), 0.0, 0).unwrap();
        assert!((mn.ndc_x - mf.ndc_x).abs() < 1e-4 && (mn.ndc_y - mf.ndc_y).abs() < 1e-4,
            "same bearing must map to the same screen point regardless of range: {mn:?} vs {mf:?}");
        // Every marker also rides the fixed edge ring (its radius is RING_RADIUS), so its distance
        // from screen-centre is constant — the position carries no range information at all.
        let r = (mn.ndc_x * mn.ndc_x + mn.ndc_y * mn.ndc_y).sqrt();
        assert!((r - RING_RADIUS).abs() < 1e-4, "marker sits on the fixed bearing ring, not a range");
    }

    /// Validate `hud.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the test suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn hud_wgsl_parses_and_validates() {
        let src = include_str!("hud.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("hud.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("hud.wgsl must validate");
    }
}
