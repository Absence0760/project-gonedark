//! End-to-end HTTP tests for the WS-D telemetry + live-ops endpoints. Drive a real `axum`
//! router with an in-memory sink — NO Docker/Postgres required, so this stays green in CI and
//! on a fresh clone (the consent-by-construction guarantee is exercised over the wire here).

use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use gonedark_server::http::{router, AppState, CONSENT_HEADER, TELEMETRY_BODY_LIMIT};
use gonedark_server::liveops::{LiveOpsSource, PersonalizedConfig, PublicConfig};
use gonedark_server::telemetry::{InMemorySink, StoreError, TelemetryEvent, TelemetrySink};
use tower::ServiceExt; // oneshot

/// A sink that always fails to store — the fixture for exercising `post_telemetry`'s `Err` arm
/// (M6), which `InMemorySink` (whose `store` always returns `Ok`) can never reach.
struct FailingSink;
impl TelemetrySink for FailingSink {
    fn store(&self, _event: TelemetryEvent) -> Result<(), StoreError> {
        Err(StoreError("boom".to_string()))
    }
}

fn event_json() -> String {
    serde_json::json!({
        "event_id": "evt-http-1",
        "kind": "embody",
        "client_ts": "2026-06-25T12:00:00Z",
        "properties": {"unit": "rifleman"}
    })
    .to_string()
}

fn state_with(sink: Arc<InMemorySink>) -> AppState {
    AppState {
        sink: sink as Arc<dyn TelemetrySink>,
        liveops: Arc::new(LiveOpsSource::new()),
    }
}

#[tokio::test]
async fn telemetry_without_consent_stores_nothing() {
    let sink = Arc::new(InMemorySink::new());
    let app = router(state_with(sink.clone()));

    // No consent header at all ⇒ default-deny ⇒ 204, nothing stored.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/telemetry")
                .header("content-type", "application/json")
                .body(Body::from(event_json()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(sink.len(), 0, "no-consent request must store nothing");
}

#[tokio::test]
async fn telemetry_with_consent_is_stored() {
    let sink = Arc::new(InMemorySink::new());
    let app = router(state_with(sink.clone()));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/telemetry")
                .header("content-type", "application/json")
                .header(CONSENT_HEADER, "true")
                .body(Body::from(event_json()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(sink.len(), 1);
    assert_eq!(sink.stored()[0].event_id, "evt-http-1");
}

/// M6: a sink failure (e.g. a Postgres pool exhausted, a dropped connection) must still surface
/// as a 500 to the client — this pins the *existing* status-code behavior — but it must not be
/// silent: this is the fixture proving the `Err` arm is actually reachable and exercised (it
/// necessarily fails before the fix existed, since `InMemorySink::store` always returns `Ok`).
#[tokio::test]
async fn telemetry_store_failure_returns_500() {
    let app = router(AppState {
        sink: Arc::new(FailingSink),
        liveops: Arc::new(LiveOpsSource::new()),
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/telemetry")
                .header("content-type", "application/json")
                .header(CONSENT_HEADER, "true")
                .body(Body::from(event_json()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn telemetry_invalid_event_is_rejected() {
    let sink = Arc::new(InMemorySink::new());
    let app = router(state_with(sink.clone()));

    let bad = serde_json::json!({
        "event_id": "",
        "kind": "embody",
        "client_ts": "2026-06-25T12:00:00Z"
    })
    .to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/telemetry")
                .header("content-type", "application/json")
                .header(CONSENT_HEADER, "true")
                .body(Body::from(bad))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(sink.len(), 0);
}

#[tokio::test]
async fn liveops_public_config_without_consent_omits_personalized() {
    let app = router(AppState::in_memory());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/liveops/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("\"public\""),
        "public config always present: {s}"
    );
    assert!(
        !s.contains("personalized"),
        "personalized config withheld without consent: {s}"
    );
}

#[tokio::test]
async fn liveops_personalized_config_present_with_consent() {
    let app = router(AppState::in_memory());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/liveops/config")
                .header(CONSENT_HEADER, "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("personalized"),
        "personalized config present with consent: {s}"
    );
}

#[tokio::test]
async fn telemetry_oversize_body_rejected_before_gate() {
    let sink = Arc::new(InMemorySink::new());
    let app = router(state_with(sink.clone()));

    // A body that exceeds the limit (a giant `properties` blob). Rejected by the body-limit
    // layer before the gate/validation ever see it — even with consent granted.
    let big = "x".repeat(TELEMETRY_BODY_LIMIT + 1024);
    let body = serde_json::json!({
        "event_id": "evt-big",
        "kind": "embody",
        "client_ts": "2026-06-25T12:00:00Z",
        "properties": {"blob": big}
    })
    .to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/telemetry")
                .header("content-type", "application/json")
                .header(CONSENT_HEADER, "true")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(sink.len(), 0, "oversize body must never reach the sink");
}

/// CP-8: a served rotation period + public track baseline round-trips over the wire even without
/// consent — the rotation *period* is public info, same footing as `maintenance`.
#[tokio::test]
async fn liveops_rotation_period_served_without_consent() {
    let liveops = Arc::new(LiveOpsSource::new().with_public(PublicConfig {
        modifier_rotation_period: Some(7),
        modifier_track: 0,
        ..PublicConfig::default()
    }));
    let app = router(AppState {
        sink: Arc::new(InMemorySink::new()),
        liveops,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/liveops/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let cfg: gonedark_server::liveops::LiveOpsConfig = serde_json::from_slice(&body).unwrap();
    assert_eq!(cfg.public.modifier_rotation_period, Some(7));
    assert_eq!(cfg.public.modifier_track, 0);
    assert_eq!(
        cfg.personalized, None,
        "no consent header ⇒ no personalized config"
    );
    assert_eq!(
        cfg.effective_modifier_track(),
        0,
        "falls back to the public track without consent"
    );
}

/// CP-8: a consent-gated experiment track override only takes effect once analytics consent is
/// granted — the HTTP-level instance of the same guarantee `liveops::tests` proves at the struct
/// level.
#[tokio::test]
async fn liveops_track_override_requires_consent_over_http() {
    let liveops = Arc::new(
        LiveOpsSource::new()
            .with_public(PublicConfig {
                modifier_rotation_period: Some(3),
                modifier_track: 0,
                ..PublicConfig::default()
            })
            .with_personalized(PersonalizedConfig {
                modifier_track_override: Some(1),
                ..PersonalizedConfig::default()
            }),
    );
    let app = router(AppState {
        sink: Arc::new(InMemorySink::new()),
        liveops,
    });

    // No consent header ⇒ effective track is the public baseline, never the override.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/liveops/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let cfg: gonedark_server::liveops::LiveOpsConfig = serde_json::from_slice(&body).unwrap();
    assert_eq!(cfg.effective_modifier_track(), 0);

    // Consent granted ⇒ the personalized override wins.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/liveops/config")
                .header(CONSENT_HEADER, "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let cfg: gonedark_server::liveops::LiveOpsConfig = serde_json::from_slice(&body).unwrap();
    assert_eq!(cfg.effective_modifier_track(), 1);
}

#[tokio::test]
async fn health_still_ok() {
    let app = router(AppState::in_memory());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
