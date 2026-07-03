//! The desktop **app shell** — native out-of-match chrome (D32) for Windows/Linux, drawn with
//! **egui** (D36: the desktop-toolkit call D32 left open). It is host-side, holds no game/sim logic,
//! and drives the shared engine only through the `core::shell` seam — the desktop counterpart of the
//! Android Jetpack-Compose shell (D35).
//!
//! Two layers, kept apart exactly like the Android shell:
//!  - a tiny **pure seam** ([`resolve_title_action`], [`build_stamp`]/[`build_channel`], and the
//!    per-screen `apply_*` action seams) — the testable decision/formatting logic, unit-tested in
//!    [`tests`] with no GPU or window;
//!  - the **egui glue** ([`EguiShell`]) — device-gated chrome (an egui context + the winit input
//!    bridge + the wgpu renderer) that draws each screen and reports the clicked action. The glue is
//!    exempt from unit tests (CLAUDE.md: thin, un-constructible-in-test platform glue), so the real
//!    logic is pushed down into the pure seam where it *is* tested.
//!
//! ## Module tree
//! This root is a thin façade: it declares the submodules and re-exports the surface `main.rs`
//! consumes. Each screen owns its pure seam + its egui builder in one file:
//!  - [`theme`] — colour ramp, type scale, the cohesive [`shell_style`];
//!  - [`widgets`] — the reusable egui component library + shared layout constants;
//!  - [`transitions`] — the title action → host-transition decision surface;
//!  - [`settings`], [`loadout`], [`profile`], [`army`], [`about`], [`pvp`], [`atlas`],
//!    [`mission_select`], [`briefing`], [`skirmish`] — one out-of-match screen each (model + seam
//!    + builder);
//!  - [`persist`] — the tolerant shell-prefs `key=value` codec;
//!  - [`util`] — the build stamp / channel / pointer→NDC helpers;
//!  - [`egui_shell`] — the [`EguiShell`] device glue + the title-screen layout.

mod about;
mod army;
mod atlas;
mod briefing;
mod egui_shell;
mod loadout;
mod mission_select;
mod persist;
mod profile;
mod pvp;
mod settings;
mod skirmish;
mod theme;
mod transitions;
mod util;
mod widgets;

// The shell's public surface for the run loop (`main.rs`). Each `pub(crate) use ...::*` both
// re-exports the API `main.rs` consumes (`use shell::{...}`) AND brings every submodule item into
// this root's namespace, so the shared test module's `use super::*` resolves the whole tree from one
// place — exactly as it did before the split. Item names are unique across submodules, so the globs
// don't collide.
pub(crate) use army::*;
pub(crate) use atlas::*;
pub(crate) use briefing::*;
pub(crate) use egui_shell::*;
pub(crate) use loadout::*;
pub(crate) use mission_select::*;
pub(crate) use persist::*;
pub(crate) use profile::*;
pub(crate) use pvp::*;
pub(crate) use settings::*;
pub(crate) use skirmish::*;
pub(crate) use transitions::*;
pub(crate) use util::*;
// `main.rs` names nothing from these three directly (the theme/widgets primitives are imported by
// sibling screens via their own `crate::shell::…` paths; About has no host-facing API). They are
// re-exported purely so the shared test module's `super::*` resolves the whole tree from one place,
// so the glob reads as unused in a non-test build — that's expected, not dead code.
#[allow(unused_imports)]
pub(crate) use about::*;
#[allow(unused_imports)]
pub(crate) use theme::*;
#[allow(unused_imports)]
pub(crate) use widgets::*;

#[cfg(test)]
mod tests;

// Dev-only headless screenshot harness for eyeballing the egui screen layouts (an #[ignore]d test).
#[cfg(test)]
mod shot;
