//! Backend services host. See docs/infrastructure.md and docs/plans/phase-4-plan.md (WS-D).
//!
//! Honors the "clone-and-run" promise: `docker compose up -d` + `cargo run -p gonedark-server`
//! gives a live HTTP service bound to `HTTP_BIND` from `.env.development`. Phase 4 WS-D adds the
//! telemetry ingest + consent gate + live-ops config endpoints — consent-respecting *by
//! construction* (a non-consenting client emits nothing; see `gonedark_server::consent`). The
//! routing/handler logic lives in the library (`gonedark_server::http`); this is just the host
//! binary. No secrets here — only the non-secret `.env.development` defaults (invariant #8); the
//! Postgres-backed sink (`DATABASE_URL`) replaces the in-memory one when that wiring lands.

use gonedark_server::http::{router, AppState};

#[tokio::main]
async fn main() {
    let bind = std::env::var("HTTP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let app = router(AppState::in_memory());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("bind HTTP_BIND");
    eprintln!("gonedark-server listening on http://{bind}");
    axum::serve(listener, app).await.expect("serve");
}
