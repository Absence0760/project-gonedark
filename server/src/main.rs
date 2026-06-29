//! Backend services host. See docs/infrastructure.md and docs/plans/phase-4-plan.md (WS-D).
//!
//! Honors the "clone-and-run" promise: `docker compose up -d` + `cargo run -p gonedark-server`
//! gives a live HTTP service bound to `HTTP_BIND` from `.env.development`. Phase 4 WS-D adds the
//! telemetry ingest + consent gate + live-ops config endpoints — consent-respecting *by
//! construction* (a non-consenting client emits nothing; see `gonedark_server::consent`). The
//! routing/handler logic lives in the library (`gonedark_server::http`); this is just the host
//! binary. No secrets here — only the non-secret `.env.development` defaults (invariant #8). The
//! Postgres-backed sink (`DATABASE_URL`) replaces the in-memory one when the binary is built
//! with `--features postgres`; without the feature (or without `DATABASE_URL`) it stays
//! in-memory so clone-and-run works with zero setup.

use gonedark_server::http::{router, AppState};

#[tokio::main]
async fn main() {
    let bind = std::env::var("HTTP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let app = router(build_state().await);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("bind HTTP_BIND");
    eprintln!("gonedark-server listening on http://{bind}");
    axum::serve(listener, app).await.expect("serve");
}

/// Choose the telemetry sink. With the `postgres` feature *and* a `DATABASE_URL` set, connect +
/// migrate the Postgres sink; otherwise fall back to in-memory. The URL is read from the env
/// only (the non-secret local default lives in `.env.development`) and is never logged, so no
/// credential is emitted (invariant #8). The sink is reached only through the consent gate
/// regardless of which one is chosen.
#[cfg(feature = "postgres")]
async fn build_state() -> AppState {
    use std::sync::Arc;
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL unset — telemetry sink: in-memory");
        return AppState::in_memory();
    };
    match gonedark_server::postgres::PgTelemetrySink::connect(&url).await {
        Ok(sink) => {
            if let Err(e) = sink.migrate().await {
                eprintln!("telemetry migrations failed ({e}) — falling back to in-memory sink");
                return AppState::in_memory();
            }
            eprintln!("telemetry sink: Postgres");
            AppState::with_sink(Arc::new(sink))
        }
        Err(e) => {
            eprintln!("Postgres connect failed ({e}) — falling back to in-memory sink");
            AppState::in_memory()
        }
    }
}

/// Without the `postgres` feature the sink is always in-memory (no `sqlx`, no database).
#[cfg(not(feature = "postgres"))]
async fn build_state() -> AppState {
    AppState::in_memory()
}
