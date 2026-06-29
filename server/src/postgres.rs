//! Postgres-backed [`TelemetrySink`] — the real storage behind the consent gate.
//! Phase 4 WS-D deferred item (docs/plans/phase-4-plan.md §4: "the real Postgres
//! `TelemetrySink`"). Server-side only; **not** in the deterministic sim path.
//!
//! # Where it sits relative to consent
//!
//! This is *only* a [`TelemetrySink`] implementation — it never touches consent itself.
//! Exactly like [`crate::telemetry::InMemorySink`], it is reachable only through
//! [`crate::telemetry::ingest`], which calls the sink **after** routing the event through
//! [`crate::consent::ConsentGate::guard`]. A non-consenting client is dropped before any sink
//! method runs, so swapping in this sink does not weaken the structural, default-deny gate —
//! the gate is upstream of the trait, not inside it.
//!
//! # Why this whole module is feature-gated (`postgres`)
//!
//! `sqlx` is an **optional** dependency enabled only by the `postgres` Cargo feature, so the
//! default build — and therefore the default `cargo test` and CI — never compiles it and needs
//! no database. Clone-and-run stays green with zero setup (invariant #8 / the CI floor): the
//! binary falls back to the in-memory sink unless built with `--features postgres` *and* given
//! a `DATABASE_URL`.
//!
//! # No compile-time database
//!
//! We use sqlx's **runtime** query API (`sqlx::query(...)`), not the `query!` compile-time
//! macros, precisely so building this module needs no live database and no checked-in `.sqlx`
//! cache. The migration SQL is embedded at compile time from `server/migrations/` via
//! [`MIGRATOR`]; embedding reads files, not a database.
//!
//! # Secrets (invariant #8)
//!
//! No credentials are hardcoded. The connection string comes from `DATABASE_URL` (the
//! committed **non-secret** local default in `.env.development`:
//! `postgres://gonedark:gonedark@localhost:5434/gonedark_dev`, the Docker `compose.yaml`
//! Postgres). Real prod credentials live only in the private estate repo via sops — never here.

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::telemetry::{StoreError, TelemetryEvent, TelemetrySink};

/// Embedded migrations (`server/migrations/`). Applied by [`PgTelemetrySink::migrate`]. Reading
/// the SQL files happens at compile time; running them needs a live database.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The single INSERT used by the sink. `ON CONFLICT (event_id) DO NOTHING` makes ingest
/// idempotent: re-delivering an event the client already sent (same `event_id`) is a no-op
/// rather than a duplicate row or an error — matching the dedupe intent of `event_id`.
///
/// Kept as a `const` so it can be asserted in a unit test without a database (query *shape* is
/// pure; only its *execution* needs Postgres).
pub const INSERT_SQL: &str = "INSERT INTO telemetry_events (event_id, kind, client_ts, properties) \
     VALUES ($1, $2, $3, $4) ON CONFLICT (event_id) DO NOTHING";

/// A [`TelemetrySink`] that persists consent-cleared events to Postgres.
///
/// Reached only through [`crate::telemetry::ingest`] (after the consent gate), identically to
/// [`crate::telemetry::InMemorySink`]. Holds a pooled connection so concurrent ingest requests
/// share connections.
#[derive(Debug, Clone)]
pub struct PgTelemetrySink {
    pool: PgPool,
}

impl PgTelemetrySink {
    /// Connect a small pool to `database_url` (e.g. the `DATABASE_URL` env var). Does **not**
    /// run migrations — call [`PgTelemetrySink::migrate`] (or run them out-of-band) so the
    /// caller controls when schema changes apply.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Wrap an already-built pool (handy for tests that share a pool / set their own options).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Apply the embedded migrations (idempotent — sqlx tracks applied versions).
    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        MIGRATOR.run(&self.pool).await
    }

    /// Borrow the underlying pool (used by integration tests to verify rows).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Async insert of one validated, consent-cleared event. The `store` trait method is sync
    /// (the existing [`TelemetrySink`] contract), so it drives this via the current Tokio
    /// runtime; this fn exists so async callers/tests can `await` directly.
    pub async fn insert(&self, event: &TelemetryEvent) -> Result<(), sqlx::Error> {
        sqlx::query(INSERT_SQL)
            .bind(&event.event_id)
            .bind(event.kind.as_db_str())
            .bind(&event.client_ts)
            .bind(&event.properties) // JSONB via the `json` feature
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

impl TelemetrySink for PgTelemetrySink {
    /// Synchronous trait method bridging to the async insert. The HTTP host runs on a
    /// multi-threaded Tokio runtime, so we hand the blocking off via `block_in_place` +
    /// `Handle::block_on` rather than nesting runtimes. Maps any sqlx error into the trait's
    /// stringly [`StoreError`] (the scaffold-level error type).
    fn store(&self, event: TelemetryEvent) -> Result<(), StoreError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.insert(&event))
        })
        .map_err(|e| StoreError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::EventKind;

    // --- Pure, no-database unit tests (run under `cargo test --features postgres`) ----------

    #[test]
    fn insert_sql_targets_the_right_table_and_columns() {
        assert!(INSERT_SQL.contains("telemetry_events"));
        for col in ["event_id", "kind", "client_ts", "properties"] {
            assert!(INSERT_SQL.contains(col), "INSERT must name column {col}");
        }
        // Four bind params, one per column — guards against a column/placeholder mismatch.
        for p in ["$1", "$2", "$3", "$4"] {
            assert!(INSERT_SQL.contains(p), "INSERT must bind {p}");
        }
    }

    #[test]
    fn insert_is_idempotent_on_event_id() {
        // Dedupe is part of the contract (event_id is the idempotency key).
        assert!(INSERT_SQL.contains("ON CONFLICT (event_id) DO NOTHING"));
    }

    #[test]
    fn migrator_has_at_least_one_migration() {
        assert!(
            MIGRATOR.iter().count() >= 1,
            "embedded migrations must be present"
        );
    }

    // --- Postgres round-trip integration test ------------------------------------------------
    //
    // GATED twice so it never runs in the default suite or CI:
    //   * the whole module is behind `--features postgres`, and
    //   * this test is `#[ignore]` — opt-in only.
    // It also reads `DATABASE_URL` and SKIPS (returns Ok) if it is unset, so even an explicit
    // `--ignored` run without a database is a no-op rather than a failure.
    //
    // To run it (needs Docker Postgres up — `docker compose up -d`):
    //   cargo test -p gonedark-server --features postgres -- --ignored postgres_round_trip
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs a live Postgres (DATABASE_URL); run with --features postgres -- --ignored"]
    async fn postgres_round_trip_stores_through_the_sink() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("DATABASE_URL unset — skipping Postgres round-trip");
            return;
        };

        let sink = PgTelemetrySink::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sink.migrate().await.expect("run migrations");

        // Unique id so reruns don't collide; ON CONFLICT also makes a rerun idempotent.
        let event_id = format!("it-{}", uuid_like());
        let event = TelemetryEvent {
            event_id: event_id.clone(),
            kind: EventKind::Embody,
            client_ts: "2026-06-29T00:00:00Z".into(),
            properties: serde_json::json!({ "unit": "rifleman", "n": 7 }),
        };

        // Drive the *sync trait method* from a blocking task so block_in_place is valid —
        // exactly how the axum handler reaches the sink.
        let sink2 = sink.clone();
        let ev2 = event.clone();
        tokio::task::spawn_blocking(move || sink2.store(ev2))
            .await
            .expect("join")
            .expect("store via trait");

        // Read it back and confirm the mapping round-tripped.
        let row: (String, String, serde_json::Value) =
            sqlx::query_as("SELECT kind, client_ts, properties FROM telemetry_events WHERE event_id = $1")
                .bind(&event_id)
                .fetch_one(sink.pool())
                .await
                .expect("fetch stored row");
        assert_eq!(EventKind::from_db_str(&row.0), Some(EventKind::Embody));
        assert_eq!(row.1, event.client_ts);
        assert_eq!(row.2["unit"], "rifleman");

        // Idempotent re-store does not create a second row.
        sink.insert(&event).await.expect("re-insert");
        let count: (i64,) =
            sqlx::query_as("SELECT count(*) FROM telemetry_events WHERE event_id = $1")
                .bind(&event_id)
                .fetch_one(sink.pool())
                .await
                .expect("count rows");
        assert_eq!(count.0, 1, "ON CONFLICT must keep this idempotent");
    }

    /// Tiny unique-ish suffix for test event ids without pulling a uuid crate.
    fn uuid_like() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
