//! The in-session shell **overlay** (Phase 4 WS-B, D32 carve-out): the pause / surrender-confirm /
//! reconnect-prompt / post-match-summary surfaces, drawn in-engine on top of the match frame.
//!
//! Like [`hud`](crate::hud), this is a screen-space LOAD pass (it composites over the already-
//! rendered frame, never clears) and a **pure presentation derivation** — it reads only the small,
//! already-presentation-safe overlay description the host hands it ([`Overlay`]) and emits
//! screen-space quads. It is checksum-neutral by construction: it never touches sim state and the
//! host computes it from `core::shell`/`engine::session_shell` views, not from `&World`.
//!
//! ## Fairness (invariant #6) is preserved structurally
//!
//! The overlay draws **opaque dim panels + bars** — chrome, not intel. It carries NO world
//! positions, no fog mask, no off-screen unit state: the post-match summary is integer counts
//! (`core::shell::MatchSummary`, all `i64`/`Fixed`), and a count is not a map reveal. Drawing the
//! overlay never widens the avatar-only fog the unit pass already applied while embodied; the dark
//! frame stays dark underneath. A summary is only ever fed in on the *ended* surface (the match is
//! over, the player is no longer embodied) — the host gates that, but even mid-match the overlay
//! has no spatial data to leak.
//!
//! The testable layout math (which panels appear, their rects, the summary bar lengths) lives in
//! the free [`overlay_quads`] so it is unit-testable without a GPU — exactly the `interpolate_
//! instances` / `marker_for` pattern.

use gonedark_core::shell::{FactionStats, MatchOutcome, MatchSummary};
use wgpu::util::DeviceExt;

/// Which in-session overlay surface the host wants drawn this frame. A flat, presentation-only
/// description — the render side never owns the session state machine (that is `engine`); it is
/// handed exactly what to draw. `Summary` carries the integer-only [`MatchSummary`] (no float, no
/// world position — invariant #1/#6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Overlay {
    /// No overlay this frame (the match is playing). Draws nothing.
    None,
    /// The pause overlay: a single dim full-screen scrim.
    Paused,
    /// The reconnect prompt: a centered panel. `desynced` picks the (here, color-coded via the
    /// quad's role) copy — stalled vs a confirmed divergence.
    ReconnectPrompt { desynced: bool },
    /// The post-match summary: a centered panel plus a per-faction bar row. Full-info, shown only
    /// after the match ends (not embodied).
    Summary(MatchSummary),
}

/// A semantic role for an overlay quad, so the shader/tint can distinguish chrome from data and a
/// test can assert *what* was drawn without pixel-matching.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuadRole {
    /// A full-screen dim scrim behind a panel (so the match reads as "paused/over").
    Scrim,
    /// A panel background.
    Panel,
    /// A neutral prompt accent (stalled reconnect).
    Accent,
    /// A warning accent (a confirmed desync — the more severe reconnect cause).
    Warning,
    /// A victory accent on the summary.
    Win,
    /// A defeat/draw accent on the summary.
    Loss,
    /// A per-faction data bar in the summary (length encodes a count — chrome, not a map).
    DataBar,
}

/// One screen-space overlay quad in NDC, ready to upload. `repr(C)` + `Pod` so it streams straight
/// into the instance buffer; the field order MUST match `overlay.wgsl`'s instance attributes and
/// the `vertex_attr_array` in [`OverlayRenderer::new`]. The `role` is CPU-side only (not uploaded).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OverlayQuad {
    /// Center in NDC ([-1,1], +y up).
    pub cx: f32,
    pub cy: f32,
    /// Half-width / half-height in NDC.
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
    /// Semantic role (CPU-side; drives the color above and lets tests assert structure).
    pub role: QuadRole,
}

/// The GPU-uploadable slice of an [`OverlayQuad`] (drops the CPU-only `role`). `repr(C)` + `Pod`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayInstance {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
    g: f32,
    b: f32,
    alpha: f32,
}

impl OverlayQuad {
    fn instance(&self) -> OverlayInstance {
        OverlayInstance {
            cx: self.cx,
            cy: self.cy,
            hw: self.hw,
            hh: self.hh,
            r: self.r,
            g: self.g,
            b: self.b,
            alpha: self.alpha,
        }
    }
}

// Layout constants (NDC). Panels are centered; the scrim spans the screen.
const SCRIM_ALPHA: f32 = 0.55;
const PANEL_HW: f32 = 0.5;
const PANEL_HH: f32 = 0.32;
/// Per-faction summary bar geometry.
const BAR_MAX_HW: f32 = 0.42; // a full bar spans most of the panel width
const BAR_HH: f32 = 0.035;
const BAR_GAP: f32 = 0.1; // vertical spacing between faction rows

fn color(role: QuadRole) -> [f32; 3] {
    match role {
        QuadRole::Scrim => [0.0, 0.0, 0.0],
        QuadRole::Panel => [0.06, 0.07, 0.10],
        QuadRole::Accent => [0.30, 0.55, 0.90], // calm blue: "waiting on a peer"
        QuadRole::Warning => [0.90, 0.25, 0.20], // red: a confirmed desync
        QuadRole::Win => [0.30, 0.80, 0.40],    // green: victory
        QuadRole::Loss => [0.70, 0.70, 0.75],   // grey: defeat/draw
        QuadRole::DataBar => [0.45, 0.65, 0.85],
    }
}

fn quad(cx: f32, cy: f32, hw: f32, hh: f32, alpha: f32, role: QuadRole) -> OverlayQuad {
    let [r, g, b] = color(role);
    OverlayQuad {
        cx,
        cy,
        hw,
        hh,
        r,
        g,
        b,
        alpha,
        role,
    }
}

/// The normalized "score" a faction's summary bar encodes — a presentation ratio computed ABOVE
/// the seam from the integer summary (invariant #1 keeps floats out of `core`; this is render-side
/// float math). Here: units killed relative to the largest kill count in the match, so the
/// best-performing side reads as a full bar. A zero-kill match yields zero-length bars (no NaN).
fn bar_fraction(stats: &FactionStats, max_kills: u32) -> f32 {
    if max_kills == 0 {
        0.0
    } else {
        (stats.units_killed as f32 / max_kills as f32).clamp(0.0, 1.0)
    }
}

/// Build the screen-space overlay quads for `overlay`. Pure (no GPU, no sim) — the testable layout
/// seam. Returns an empty vec for [`Overlay::None`]. Quads are returned back-to-front (scrim first,
/// then panel, then accents/bars) so an alpha-blended LOAD pass composites correctly.
pub fn overlay_quads(overlay: &Overlay) -> Vec<OverlayQuad> {
    match overlay {
        Overlay::None => Vec::new(),
        Overlay::Paused => {
            // A single dim scrim across the whole screen + a small "paused" panel.
            vec![
                quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim),
                quad(0.0, 0.0, PANEL_HW, PANEL_HH, 0.92, QuadRole::Panel),
            ]
        }
        Overlay::ReconnectPrompt { desynced } => {
            let accent = if *desynced {
                QuadRole::Warning
            } else {
                QuadRole::Accent
            };
            vec![
                quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim),
                quad(0.0, 0.0, PANEL_HW, PANEL_HH, 0.92, QuadRole::Panel),
                // An accent strip across the top of the panel signals the cause (blue/red).
                quad(0.0, PANEL_HH - 0.04, PANEL_HW, 0.04, 1.0, accent),
            ]
        }
        Overlay::Summary(summary) => {
            let mut out = vec![
                quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim),
                quad(0.0, 0.0, PANEL_HW, PANEL_HH, 0.95, QuadRole::Panel),
            ];
            // Outcome accent strip across the top of the panel.
            let outcome_role = match summary.outcome {
                MatchOutcome::Victory(_) => QuadRole::Win,
                MatchOutcome::Draw => QuadRole::Loss,
            };
            out.push(quad(
                0.0,
                PANEL_HH - 0.04,
                PANEL_HW,
                0.04,
                1.0,
                outcome_role,
            ));

            // Per-faction kill bars, top-down inside the panel. Bar length encodes kills relative
            // to the match max — a presentation ratio, never a spatial reveal.
            let max_kills = summary
                .per_faction
                .iter()
                .map(|s| s.units_killed)
                .max()
                .unwrap_or(0);
            // Start the rows below the accent strip and lay them out downward.
            let top = PANEL_HH - 0.14;
            for (row, stats) in summary.per_faction.iter().enumerate() {
                let frac = bar_fraction(stats, max_kills);
                let hw = (BAR_MAX_HW * frac).max(0.0);
                let cy = top - row as f32 * BAR_GAP;
                // Anchor bars to the panel's left edge so length reads left-to-right; a zero-length
                // bar is skipped (nothing to draw) but the row slot is still consumed.
                if hw > 0.0 {
                    let left = -BAR_MAX_HW;
                    out.push(quad(left + hw, cy, hw, BAR_HH, 1.0, QuadRole::DataBar));
                }
            }
            out
        }
    }
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-quad half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

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

const INITIAL_CAP: usize = 16;

/// Screen-space in-session shell overlay renderer. Owns its own pipeline + buffers (a separate
/// pipeline from the unit + HUD passes so the three never contend for a shader). Alpha-blended LOAD
/// pass: composites over the already-rendered (possibly dark) frame.
pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl OverlayRenderer {
    /// Build the overlay pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Alpha blending so panels dim the frame beneath.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.overlay_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.overlay_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<OverlayInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec3), 4=alpha(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x3,
                4 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.overlay_pipeline"),
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
            label: Some("gonedark.overlay_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.overlay_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<OverlayInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        OverlayRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the in-session overlay on top of `view` (a LOAD pass — never clears). Builds the quad
    /// set via [`overlay_quads`], uploads it, and records one LOAD render pass so the overlay
    /// composites over the (possibly dark) match frame. No-op for [`Overlay::None`].
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        overlay: &Overlay,
    ) {
        let quads = overlay_quads(overlay);
        if quads.is_empty() {
            return;
        }
        let instances: Vec<OverlayInstance> = quads.iter().map(|q| q.instance()).collect();

        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.overlay_instance_vbo"),
                size: (new_cap * std::mem::size_of::<OverlayInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.overlay_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.overlay_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
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
            pass.draw(0..QUAD_VERTS.len() as u32, 0..instances.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `OverlayRenderer::new`
    //! needs a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the
    //! testable layout math is factored into [`overlay_quads`].

    use super::*;
    use gonedark_core::components::{Faction, FACTION_COUNT};
    use gonedark_core::shell::{FactionStats, MatchOutcome, MatchSummary};

    fn roles(quads: &[OverlayQuad]) -> Vec<QuadRole> {
        quads.iter().map(|q| q.role).collect()
    }

    fn summary_with_kills(player: u32, enemy: u32, outcome: MatchOutcome) -> MatchSummary {
        let mut per_faction: [FactionStats; FACTION_COUNT] = Default::default();
        for f in Faction::ALL {
            per_faction[f.index()].faction = f.into();
        }
        per_faction[Faction::Player.index()].units_killed = player;
        per_faction[Faction::Enemy.index()].units_killed = enemy;
        MatchSummary {
            outcome,
            end_tick: 3600,
            per_faction,
        }
    }

    #[test]
    fn none_draws_nothing() {
        assert!(overlay_quads(&Overlay::None).is_empty());
    }

    #[test]
    fn paused_is_scrim_plus_panel() {
        let q = overlay_quads(&Overlay::Paused);
        assert_eq!(roles(&q), vec![QuadRole::Scrim, QuadRole::Panel]);
        // The scrim spans the whole screen; the panel is centered and smaller.
        assert_eq!((q[0].hw, q[0].hh), (1.0, 1.0));
        assert!(q[1].hw < 1.0 && q[1].hh < 1.0);
        assert_eq!((q[1].cx, q[1].cy), (0.0, 0.0));
    }

    #[test]
    fn reconnect_stalled_uses_accent_desynced_uses_warning() {
        let stalled = overlay_quads(&Overlay::ReconnectPrompt { desynced: false });
        assert!(roles(&stalled).contains(&QuadRole::Accent));
        assert!(!roles(&stalled).contains(&QuadRole::Warning));

        let desync = overlay_quads(&Overlay::ReconnectPrompt { desynced: true });
        assert!(roles(&desync).contains(&QuadRole::Warning));
        assert!(!roles(&desync).contains(&QuadRole::Accent));
    }

    #[test]
    fn summary_victory_uses_win_accent_draw_uses_loss() {
        let win = overlay_quads(&Overlay::Summary(summary_with_kills(
            5,
            2,
            MatchOutcome::Victory(Faction::Player),
        )));
        assert!(roles(&win).contains(&QuadRole::Win));
        assert!(!roles(&win).contains(&QuadRole::Loss));

        let draw = overlay_quads(&Overlay::Summary(summary_with_kills(
            0,
            0,
            MatchOutcome::Draw,
        )));
        assert!(roles(&draw).contains(&QuadRole::Loss));
        assert!(!roles(&draw).contains(&QuadRole::Win));
    }

    #[test]
    fn summary_bar_length_tracks_relative_kills() {
        // Player 4 kills, enemy 2 kills → player bar is full width, enemy bar half.
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            4,
            2,
            MatchOutcome::Victory(Faction::Player),
        )));
        let bars: Vec<&OverlayQuad> = q.iter().filter(|q| q.role == QuadRole::DataBar).collect();
        assert_eq!(bars.len(), 2, "two non-zero faction bars (neutral has 0)");
        // Player row is first (rows are in Faction::ALL order). Its half-width is the max.
        assert!((bars[0].hw - BAR_MAX_HW).abs() < 1e-5, "leader is a full bar");
        assert!(
            (bars[1].hw - BAR_MAX_HW * 0.5).abs() < 1e-5,
            "half the kills → half the bar"
        );
    }

    #[test]
    fn summary_zero_kills_draws_no_bars_no_nan() {
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            0,
            0,
            MatchOutcome::Draw,
        )));
        let bars = q.iter().filter(|q| q.role == QuadRole::DataBar).count();
        assert_eq!(bars, 0, "no kills → no bars (and no division-by-zero NaN)");
        for q in &q {
            assert!(q.hw.is_finite() && q.hh.is_finite());
        }
    }

    #[test]
    fn bar_fraction_is_safe_at_zero_max() {
        let stats = FactionStats {
            units_killed: 0,
            ..Default::default()
        };
        assert_eq!(bar_fraction(&stats, 0), 0.0);
    }

    /// Every overlay surface that draws starts with a full-screen scrim — the match reads as
    /// interrupted/over beneath it, and nothing of the live frame peeks through as "intel".
    #[test]
    fn every_drawn_surface_dims_the_frame_first() {
        for ov in [
            Overlay::Paused,
            Overlay::ReconnectPrompt { desynced: false },
            Overlay::ReconnectPrompt { desynced: true },
            Overlay::Summary(summary_with_kills(
                1,
                0,
                MatchOutcome::Victory(Faction::Player),
            )),
        ] {
            let q = overlay_quads(&ov);
            assert_eq!(q[0].role, QuadRole::Scrim, "first quad is the scrim for {ov:?}");
            assert_eq!((q[0].hw, q[0].hh), (1.0, 1.0));
        }
    }

    /// Fairness guard (invariant #6): no overlay quad carries a world position — every quad is in
    /// NDC and bounded to the screen. The overlay has no spatial sim data to leak.
    #[test]
    fn overlay_quads_are_screen_space_only() {
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            3,
            1,
            MatchOutcome::Victory(Faction::Player),
        )));
        for quad in &q {
            assert!(quad.cx >= -1.5 && quad.cx <= 1.5, "cx in NDC range");
            assert!(quad.cy >= -1.5 && quad.cy <= 1.5, "cy in NDC range");
        }
    }

    #[test]
    fn overlay_wgsl_parses_and_validates() {
        let src = include_str!("overlay.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("overlay.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("overlay.wgsl must validate");
    }
}
