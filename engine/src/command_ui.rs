//! The order/stance command vocabulary on a small screen (roadmap Phase 2: "the real depth
//! layer", game-design §8). This is where "smart play" lives — NOT in the unit AI (invariant
//! #3: units are literal executors). This layer turns an on-screen vocabulary choice
//! ([`InputFrame::command_slot`] / [`InputFrame::long_press`]) plus the current [`Selection`]
//! plus the tapped world point into the `Command`s the deterministic sim already understands.
//!
//! It is pure presentation→intent mapping: it emits `Command`s, it never mutates sim state, and
//! the float→`Fixed` quantization for any world target goes through the engine's input-boundary
//! [`crate::world_to_fixed`] (invariant #1). No GPU/camera dependency → unit-testable.
//!
//! IMPLEMENTATION OWNER: worker 5 (order/stance vocabulary). Compiling stub: emits no commands,
//! so the engine keeps its legacy single-unit behavior until you fill `commands_for` + inline
//! tests. KEEP the public signature intact; you own the slot→action vocabulary mapping (and
//! should document the slot numbering you choose in a module doc comment).

use crate::selection::Selection;
use gonedark_core::sim::Command;

/// Map this frame's command-UI intent onto sim commands for the current selection.
///
/// - `command_slot`: the vocabulary button pressed this frame (you define the numbering, e.g.
///   0 = Move, 1 = Attack-move, 2 = Patrol, 3 = Hold position, 4 = cycle stance …).
/// - `long_press`: the "open context / confirm" edge.
/// - `selection`: the units the action applies to (may be empty → emit nothing).
/// - `target_world`: the world point tapped this frame, if any (for Move/AttackMove/Patrol).
///
/// Quantize any world target with [`crate::world_to_fixed`]. KEEP this signature; the body is
/// yours.
pub fn commands_for(
    _command_slot: Option<u8>,
    _long_press: bool,
    _selection: &Selection,
    _target_world: Option<(f32, f32)>,
) -> Vec<Command> {
    Vec::new()
}
