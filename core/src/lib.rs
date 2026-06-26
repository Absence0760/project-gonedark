//! gonedark-core — the shared deterministic simulation.
//!
//! Load-bearing invariants (CLAUDE.md / docs/architecture.md), all structural here:
//! - **#1 No floats in the sim.** Everything is [`fixed::Fixed`] Q16.16; there is no
//!   float conversion in `core`, so a stray float does not compile.
//! - **#2 No platform deps.** This crate pulls no GPU/windowing/OS crate (see Cargo.toml).
//! - **#4 Sim/render decoupled.** The renderer consumes [`snapshot::Snapshot`] and never
//!   mutates sim state.
//! - **#7 Per-tick checksum.** [`sim::Sim::checksum`] folds state for the CI desync matrix.
//!
//! See docs/phase-1-plan.md for the build order this scaffolds.
#![forbid(unsafe_code)]

pub mod alerts;
pub mod checksum;
pub mod combat;
pub mod commander;
pub mod components;
pub mod detection;
pub mod economy;
pub mod ecs;
pub mod event;
pub mod fixed;
pub mod flow_field;
pub mod fog;
pub mod lockstep;
pub mod orders;
pub mod persist;
pub mod projectile;
pub mod reconnect;
pub mod rng;
pub mod scenario;
pub mod shell;
pub mod sim;
pub mod snapshot;
pub mod spatial;
pub mod systems;
pub mod terrain;
pub mod territory;
pub mod trig;

pub use fixed::Fixed;
pub use sim::{Command, Sim, TICK_HZ};

#[cfg(test)]
mod tests;
