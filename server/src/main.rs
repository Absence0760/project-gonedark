//! Backend services host (placeholder). See docs/infrastructure.md.
//!
//! Phase 1 ships single-player on-device, so there is no matchmaker/relay/accounts yet.
//! This exists to honor the "clone-and-run" promise: `docker compose up -d` + `cargo run
//! -p gonedark-server` gives a live health endpoint bound to `HTTP_BIND` from
//! `.env.development`. The service split (matchmaker, accounts, telemetry, store/
//! entitlements — Q9) comes when there is logic to put behind the seams.

use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    let bind = std::env::var("HTTP_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let app = Router::new()
        .route("/health", get(health))
        .route("/", get(root));

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("bind HTTP_BIND");
    eprintln!("gonedark-server listening on http://{bind}");
    axum::serve(listener, app).await.expect("serve");
}

async fn health() -> &'static str {
    "ok"
}

async fn root() -> &'static str {
    "going-dark backend (placeholder — see docs/infrastructure.md)"
}
