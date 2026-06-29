//! HTTP surface — the router, app state, and handlers for the telemetry ingest endpoint and
//! the live-ops config endpoint. Phase 4 WS-D (docs/plans/phase-4-plan.md §4).
//!
//! Consent travels per-request in the `X-Consent-Analytics` header (the native consent screen,
//! surface 8 — deferred — will set it; until then any non-consenting/absent value denies). The
//! header is parsed by the pure [`parse_consent`] so the gate decision is unit-testable without
//! spinning up HTTP. Every handler builds its [`ConsentGate`] from that parse and funnels the
//! emit through the gated paths in [`crate::telemetry`] / [`crate::liveops`].

use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};

use crate::consent::{ConsentGate, ConsentState};
use crate::liveops::{LiveOpsConfig, LiveOpsSource};
use crate::telemetry::{ingest, Ingest, InMemorySink, TelemetryEvent, TelemetrySink};

/// Header carrying the client's analytics-consent decision. `"true"`/`"1"` ⇒ granted; anything
/// else (including absent) ⇒ denied. The default-deny posture is the safe one for privacy and
/// the no-secrets / clone-and-run stance (invariant #8): unknown consent never emits.
pub const CONSENT_HEADER: &str = "x-consent-analytics";

/// Max request body for the telemetry ingest endpoint. `properties` is free-form JSON and the
/// `Json` extractor buffers the whole body *before* the gate/validation run, so a public-facing
/// scaffold needs a ceiling to avoid an unbounded-buffer DoS. 64 KiB is generous for a single
/// event; bump it deliberately if a real batched-ingest schema arrives.
pub const TELEMETRY_BODY_LIMIT: usize = 64 * 1024;

/// Pure consent parse from the request headers — the whole gate decision, testable without
/// HTTP. Absent or unrecognised ⇒ [`ConsentState::DENIED`].
pub fn parse_consent(headers: &HeaderMap) -> ConsentState {
    let granted = headers
        .get(CONSENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let s = s.trim();
            s.eq_ignore_ascii_case("true") || s == "1"
        })
        .unwrap_or(false);
    if granted {
        ConsentState::analytics_granted()
    } else {
        ConsentState::DENIED
    }
}

/// Shared application state. Both the sink and the live-ops source are trait/struct seams so a
/// real Postgres/Redis-backed implementation drops in without touching handlers.
#[derive(Clone)]
pub struct AppState {
    pub sink: Arc<dyn TelemetrySink>,
    pub liveops: Arc<LiveOpsSource>,
}

impl AppState {
    /// Default state for local/dev: an in-memory sink + default live-ops config. The
    /// Postgres-backed sink replaces `sink` via [`AppState::with_sink`] when `DATABASE_URL`
    /// wiring is enabled (the `postgres` feature).
    pub fn in_memory() -> Self {
        AppState {
            sink: Arc::new(InMemorySink::new()),
            liveops: Arc::new(LiveOpsSource::new()),
        }
    }

    /// State backed by a caller-provided sink (e.g. the Postgres sink) + default live-ops.
    /// The sink is still reached only through the consent-gated [`ingest`] path — swapping the
    /// implementation here does not change the gate.
    pub fn with_sink(sink: Arc<dyn TelemetrySink>) -> Self {
        AppState {
            sink,
            liveops: Arc::new(LiveOpsSource::new()),
        }
    }
}

/// Build the full router (health + telemetry + live-ops). The telemetry ingest route carries a
/// per-route body-size cap ([`TELEMETRY_BODY_LIMIT`]) so an over-limit body is rejected before
/// it is buffered — the gate/validation never see it.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/", get(root))
        .route(
            "/v1/telemetry",
            post(post_telemetry).layer(DefaultBodyLimit::max(TELEMETRY_BODY_LIMIT)),
        )
        .route("/v1/liveops/config", get(get_liveops_config))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn root() -> &'static str {
    "going-dark backend (telemetry + consent gate scaffold — docs/plans/phase-4-plan.md WS-D)"
}

/// `POST /v1/telemetry` — ingest one event, consent-gated. Returns:
/// - `202 Accepted` when consent present + valid (stored),
/// - `204 No Content` when no consent (no-op at the source: nothing stored, and we don't
///   leak that to the client as an error — the absence is the correct, quiet outcome),
/// - `400 Bad Request` on validation failure.
async fn post_telemetry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<TelemetryEvent>,
) -> impl IntoResponse {
    let gate = ConsentGate::new(parse_consent(&headers));
    match ingest(gate, state.sink.as_ref(), event) {
        Ok(Ingest::Stored) => StatusCode::ACCEPTED,
        Ok(Ingest::NoConsent) => StatusCode::NO_CONTENT,
        Ok(Ingest::Invalid(_)) => StatusCode::BAD_REQUEST,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `GET /v1/liveops/config` — public config always; personalized config only with consent.
async fn get_liveops_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<LiveOpsConfig> {
    let gate = ConsentGate::new(parse_consent(&headers));
    Json(state.liveops.resolve(gate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(consent: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(CONSENT_HEADER, HeaderValue::from_str(consent).unwrap());
        h
    }

    #[test]
    fn absent_header_denies() {
        assert_eq!(parse_consent(&HeaderMap::new()), ConsentState::DENIED);
    }

    #[test]
    fn true_header_grants() {
        assert!(parse_consent(&headers_with("true")).allows_analytics());
        assert!(parse_consent(&headers_with("TRUE")).allows_analytics());
        assert!(parse_consent(&headers_with(" 1 ")).allows_analytics());
    }

    #[test]
    fn other_values_deny() {
        for v in ["false", "0", "yes", "", "maybe"] {
            assert_eq!(
                parse_consent(&headers_with(v)),
                ConsentState::DENIED,
                "value {v:?} must deny"
            );
        }
    }
}
