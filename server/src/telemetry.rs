//! Telemetry pipeline (scaffold) — event schema + a consent-gated ingest/emit path.
//! Phase 4 WS-D step 2 (docs/plans/phase-4-plan.md §4). Server-side only; **not** in the
//! deterministic sim path (no `core`/`engine` deps), so there is no checksum concern.
//!
//! Every emit path here funnels through [`crate::consent::ConsentGate`] so a non-consenting
//! client emits nothing — "no-op at the source", not "emit then filter". See
//! [`crate::consent`] for the structural argument.
//!
//! This is a **scaffold**: schema + ingest + a storage seam. The real analytics product
//! (aggregation, retention, the Postgres schema migrations) is later. Storage is modelled as
//! a [`TelemetrySink`] trait so the gate/ingest logic is testable with an in-memory fake and
//! `cargo test` stays green WITHOUT Docker/Postgres running (clone-and-run / CI floor).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::consent::ConsentGate;

/// The known telemetry event kinds. A closed enum (not a free-form string) so the schema is
/// the contract: the ingest endpoint rejects anything it doesn't recognise, and the storage
/// layer can evolve a typed table per kind. Scaffold-level — extend as real events are
/// defined. `snake_case` on the wire to match the JSON convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// App reached the main menu / shell (a session start signal).
    SessionStart,
    /// App backgrounded / shell exited.
    SessionEnd,
    /// A match was entered (post match-setup).
    MatchStart,
    /// A match concluded (carries result via `properties`, never sim-internal state).
    MatchEnd,
    /// Player possessed a unit (the going-dark beat) — a key product signal (invariant #6).
    Embody,
    /// Player was ejected back to command (embodied unit died / surfaced).
    Surface,
}

impl EventKind {
    /// Stable string for the `kind` column of the Postgres `telemetry_events` table (and any
    /// other text serialization). Kept identical to the `snake_case` JSON wire form so the DB
    /// value matches what the client sent — one vocabulary, not two. Pure + unit-tested so the
    /// Postgres sink's row mapping is covered without a database (see the `as_db_str`/
    /// `from_db_str` round-trip in this module's tests).
    pub const fn as_db_str(self) -> &'static str {
        match self {
            EventKind::SessionStart => "session_start",
            EventKind::SessionEnd => "session_end",
            EventKind::MatchStart => "match_start",
            EventKind::MatchEnd => "match_end",
            EventKind::Embody => "embody",
            EventKind::Surface => "surface",
        }
    }

    /// Inverse of [`EventKind::as_db_str`] — parse a `kind` column back to the typed enum.
    /// Returns `None` for an unknown string (a row written by a newer schema), so a reader can
    /// decide how to handle a forward-incompatible row rather than silently mismapping it.
    pub fn from_db_str(s: &str) -> Option<EventKind> {
        Some(match s {
            "session_start" => EventKind::SessionStart,
            "session_end" => EventKind::SessionEnd,
            "match_start" => EventKind::MatchStart,
            "match_end" => EventKind::MatchEnd,
            "embody" => EventKind::Embody,
            "surface" => EventKind::Surface,
            _ => return None,
        })
    }
}

/// One telemetry event as it arrives on the wire and is stored. Deliberately flat and
/// schema-stable: a typed [`EventKind`], an opaque client-supplied id + timestamp (the server
/// does not mint these — keeps the scaffold dependency-free and lets the client dedupe), and a
/// free-form JSON `properties` bag for kind-specific detail.
///
/// **No secrets, no PII by construction of the scaffold:** `properties` is opaque JSON the
/// client controls; the schema names no identifying field. The future consent screen governs
/// whether anything is sent at all (the gate), which is the privacy contract for this phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEvent {
    /// Client-generated unique id for this event (for idempotent ingest / dedupe).
    pub event_id: String,
    /// The typed event kind.
    pub kind: EventKind,
    /// Client wall-clock at emit, ISO-8601 string. Opaque to the scaffold (no time dep).
    pub client_ts: String,
    /// Kind-specific detail. Opaque JSON; defaults to `{}`.
    #[serde(default)]
    pub properties: serde_json::Value,
}

/// Validation outcome for an incoming event, kept separate from transport concerns so it is
/// unit-testable without HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// `event_id` was empty.
    EmptyEventId,
    /// `client_ts` was empty.
    EmptyTimestamp,
}

impl TelemetryEvent {
    /// Minimal structural validation. Schema-level (serde) already guarantees a known
    /// `kind`; this catches empties the type system can't. Scaffold-level — real validation
    /// (timestamp parsing, property-schema-per-kind) is later.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.event_id.trim().is_empty() {
            return Err(ValidationError::EmptyEventId);
        }
        if self.client_ts.trim().is_empty() {
            return Err(ValidationError::EmptyTimestamp);
        }
        Ok(())
    }
}

/// Storage seam. The real implementation writes to the local Docker Postgres
/// (`DATABASE_URL`, docs/infrastructure.md); tests use [`InMemorySink`] so the suite needs no
/// running database. Kept tiny on purpose — this is a scaffold seam, not an ORM.
pub trait TelemetrySink: Send + Sync {
    /// Persist one validated, consent-cleared event. Returns the number stored (1).
    fn store(&self, event: TelemetryEvent) -> Result<(), StoreError>;
}

/// Storage failure (e.g. DB unavailable). Stringly-typed at the scaffold stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

/// In-memory sink for tests and local smoke runs. Records every stored event so a test can
/// assert exactly what was (and wasn't) emitted.
#[derive(Debug, Default)]
pub struct InMemorySink {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl InMemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of everything stored so far.
    pub fn stored(&self) -> Vec<TelemetryEvent> {
        self.events.lock().expect("telemetry sink mutex").clone()
    }

    /// Count of stored events — the assertion most tests want.
    pub fn len(&self) -> usize {
        self.events.lock().expect("telemetry sink mutex").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TelemetrySink for InMemorySink {
    fn store(&self, event: TelemetryEvent) -> Result<(), StoreError> {
        self.events.lock().expect("telemetry sink mutex").push(event);
        Ok(())
    }
}

/// The outcome of an ingest attempt — distinguishes the three paths a test cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingest {
    /// Consent present + event valid: stored.
    Stored,
    /// No consent: nothing emitted, nothing stored — the consent-by-construction no-op.
    NoConsent,
    /// Consent present but the event failed validation.
    Invalid(ValidationError),
}

/// **The single consent-gated emit/ingest path.** Validation runs first, then the event is
/// passed through [`ConsentGate::guard`]: under no consent the gate yields `None` and we
/// return [`Ingest::NoConsent`] *without ever calling the sink* — the store is never touched,
/// so nothing is emitted at the source. This is the one function both the HTTP handler and any
/// future internal emitter call; there is no other way to reach the sink.
pub fn ingest(
    gate: ConsentGate,
    sink: &dyn TelemetrySink,
    event: TelemetryEvent,
) -> Result<Ingest, StoreError> {
    if let Err(e) = event.validate() {
        return Ok(Ingest::Invalid(e));
    }
    match gate.guard(event) {
        None => Ok(Ingest::NoConsent),
        Some(ev) => {
            sink.store(ev)?;
            Ok(Ingest::Stored)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::ConsentState;

    fn sample(kind: EventKind) -> TelemetryEvent {
        TelemetryEvent {
            event_id: "evt-1".into(),
            kind,
            client_ts: "2026-06-25T00:00:00Z".into(),
            properties: serde_json::json!({}),
        }
    }

    #[test]
    fn no_consent_stores_nothing() {
        // The central guarantee for the pipeline: a non-consenting client emits zero events.
        let sink = InMemorySink::new();
        let gate = ConsentGate::denied();
        let out = ingest(gate, &sink, sample(EventKind::Embody)).unwrap();
        assert_eq!(out, Ingest::NoConsent);
        assert_eq!(sink.len(), 0, "sink must be untouched without consent");
    }

    #[test]
    fn consent_flows_to_store() {
        let sink = InMemorySink::new();
        let gate = ConsentGate::new(ConsentState::analytics_granted());
        let out = ingest(gate, &sink, sample(EventKind::MatchStart)).unwrap();
        assert_eq!(out, Ingest::Stored);
        assert_eq!(sink.len(), 1);
        assert_eq!(sink.stored()[0].kind, EventKind::MatchStart);
    }

    #[test]
    fn invalid_event_rejected_even_with_consent() {
        let sink = InMemorySink::new();
        let gate = ConsentGate::new(ConsentState::analytics_granted());
        let mut ev = sample(EventKind::SessionStart);
        ev.event_id = "  ".into();
        let out = ingest(gate, &sink, ev).unwrap();
        assert_eq!(out, Ingest::Invalid(ValidationError::EmptyEventId));
        assert_eq!(sink.len(), 0);
    }

    #[test]
    fn validation_runs_before_consent_check() {
        // Submit an invalid event to a *granted* gate: if consent were checked first the
        // event would be Stored. Reporting Invalid (and storing nothing) proves validation
        // runs ahead of the consent gate.
        let sink = InMemorySink::new();
        let gate = ConsentGate::new(ConsentState::analytics_granted());
        let mut ev = sample(EventKind::SessionEnd);
        ev.client_ts = "".into();
        let out = ingest(gate, &sink, ev).unwrap();
        assert_eq!(out, Ingest::Invalid(ValidationError::EmptyTimestamp));
        assert_eq!(sink.len(), 0);
    }

    #[test]
    fn event_roundtrips_through_json() {
        let ev = sample(EventKind::Surface);
        let s = serde_json::to_string(&ev).unwrap();
        let back: TelemetryEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
        // wire form uses snake_case kinds
        assert!(s.contains("\"surface\""));
    }

    #[test]
    fn unknown_event_kind_is_rejected_at_schema() {
        let bad = r#"{"event_id":"x","kind":"not_a_kind","client_ts":"t"}"#;
        assert!(serde_json::from_str::<TelemetryEvent>(bad).is_err());
    }

    #[test]
    fn db_str_round_trips_for_every_kind() {
        // The pure row mapping the Postgres sink relies on: every kind must survive
        // as_db_str -> from_db_str unchanged. Exhaustive so a new variant fails this test
        // until its mapping is added (the `match` in as_db_str also forces a compile error).
        for kind in [
            EventKind::SessionStart,
            EventKind::SessionEnd,
            EventKind::MatchStart,
            EventKind::MatchEnd,
            EventKind::Embody,
            EventKind::Surface,
        ] {
            assert_eq!(
                EventKind::from_db_str(kind.as_db_str()),
                Some(kind),
                "{kind:?} must round-trip through its db string"
            );
        }
    }

    #[test]
    fn db_str_matches_json_wire_form() {
        // One vocabulary: the DB `kind` string equals the snake_case JSON tag, so a value
        // written by the sink reads back as what the client sent.
        let ev = sample(EventKind::MatchEnd);
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], ev.kind.as_db_str());
    }

    #[test]
    fn from_db_str_rejects_unknown() {
        assert_eq!(EventKind::from_db_str("not_a_kind"), None);
        assert_eq!(EventKind::from_db_str(""), None);
    }
}
