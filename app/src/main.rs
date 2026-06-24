//! Host: wires core + pal + render and owns the run loop / sim–render split.
//!
//! This scaffolds the four Phase-1 seams (docs/phase-1-plan.md §3, build-order step 5):
//!  - a **fixed-tick accumulator** advancing the deterministic sim (invariant #4),
//!  - **render interpolation** between the last two snapshots (invariant #4),
//!  - the **embodiment input-source swap** (invariant #5) — possess/release one entity,
//!  - the **avatar-local-prediction seam** (D15) — prediction is computed in the
//!    PRESENTATION path and never written back into sim state.
//!
//! Time and rendering are stubbed (headless desktop backend, one tick per frame) until the
//! winit+wgpu backend lands (step 4); the *structure* is the deliverable.

use gonedark_core::components::Vec2;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::{Command, Sim, TICK_HZ};
use gonedark_core::snapshot::Snapshot;
use gonedark_pal::{Input, InputFrame, Window};
use gonedark_pal_desktop::{DesktopInput, DesktopWindow};
use gonedark_render::Renderer;

fn main() {
    let mut window = DesktopWindow::new(1280, 720, 180);
    let mut input = DesktopInput::default();
    let mut renderer = Renderer::new();

    let mut sim = Sim::new(0x00C0FFEE);
    let player = sim.world.spawn();
    sim.step(&[Command::Move {
        entity: player,
        target: Vec2::new(Fixed::from_int(20), Fixed::from_int(8)),
    }]);

    let mut curr = sim.snapshot();

    // Real loop drains an accumulator: `acc += dt; while acc >= TICK { step(); acc -= TICK }`.
    // Faked to one tick per frame here; the fixed dt is derived from the (provisional) rate.
    let _tick_dt = 1.0_f32 / TICK_HZ as f32;
    let mut embodied = false;

    while window.pump() {
        let frame = input.poll();

        // --- embodiment seam (invariant #5): swap the possessed unit's input source ---
        if frame.embody_pressed && !embodied {
            sim.step(&[Command::Embody { entity: player }]);
            embodied = true;
            eprintln!("[tick {}] EMBODY — world goes dark", sim.tick_count());
        } else if frame.surface_pressed && embodied {
            sim.step(&[Command::Surface { entity: player }]);
            embodied = false;
            eprintln!("[tick {}] SURFACE — back to command", sim.tick_count());
        }

        // --- one deterministic tick (real loop would drain the accumulator) ---
        let prev = curr.clone();
        sim.step(&[]);
        curr = sim.snapshot();

        // --- avatar-local prediction seam (D15): presentation-only, never writes sim ---
        predict_avatar(&curr, &frame, embodied);

        // --- render: interpolate prev→curr (alpha faked to 1.0 in this stub) ---
        renderer.prepare(&prev, &curr, 1.0);
        renderer.draw();
    }

    eprintln!(
        "ran {} ticks; final checksum {:016x}",
        sim.tick_count(),
        sim.checksum()
    );
}

/// Avatar-local prediction (D15) lives HERE, in the presentation path. It reads sim state
/// plus the latest input to predict the embodied unit's transform for a responsive local
/// view, and MUST NOT feed back into the sim (or lockstep desyncs silently — invariant #1).
/// Authoritative resolution still happens in the sim at tick T+D. Stub for Phase 1.
fn predict_avatar(_snapshot: &Snapshot, _frame: &InputFrame, _embodied: bool) {
    // TODO(phase3): integrate local aim/move from `_frame`; reconcile against the tick.
}
