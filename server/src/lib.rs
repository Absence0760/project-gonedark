//! `gonedark-server` library seam — backend services for Going Dark (docs/infrastructure.md).
//!
//! Phase 4 WS-D (docs/plans/phase-4-plan.md §4) lands the **telemetry + consent-gated live-ops
//! scaffolding** here, server-side only. The load-bearing property is **consent by
//! construction**: a non-consenting client emits *nothing* (no-op at the source), enforced
//! structurally by [`consent::ConsentGate`] which sits on every emit path. See [`consent`].
//!
//! This is a **scaffold**, not a finished analytics/live-ops product. Storage is wired behind
//! a [`telemetry::TelemetrySink`] trait so the suite tests the gate + ingest logic with an
//! in-memory fake — `cargo test` stays green WITHOUT Docker/Postgres running (CI floor /
//! clone-and-run). A real Postgres-backed sink (`DATABASE_URL`, docs/infrastructure.md) slots
//! in behind the same trait later (the optional `postgres` feature lands exactly that — see
//! [`postgres`]). No `core`/`engine` deps leak in (server is not in the deterministic sim path;
//! invariant #2 layering stays clean), and no secret is committed anywhere (invariant #8) —
//! only the non-secret `.env.development` defaults are read.

pub mod consent;
pub mod http;
pub mod liveops;
pub mod telemetry;

/// Postgres-backed [`telemetry::TelemetrySink`] — the deferred WS-D storage. Behind the
/// optional `postgres` feature so the default build/test/CI needs no `sqlx` and no database
/// (clone-and-run floor). It is reached only through the consent gate, exactly like the
/// in-memory sink; the feature swaps the implementation, never the gate. See [`postgres`].
#[cfg(feature = "postgres")]
pub mod postgres;
