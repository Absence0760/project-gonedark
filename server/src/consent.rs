//! Consent gate — the single authoritative consent representation that BOTH telemetry
//! and (later) the store check. Phase 4 WS-D, surface 8 (docs/plans/phase-4-plan.md §2, §4).
//!
//! # Consent by construction
//!
//! The load-bearing property of this whole workstream: **a non-consenting client emits
//! nothing.** Not "emit then filter on the server" — *nothing leaves the source*. We make
//! that structural by routing every emit path through [`ConsentGate::guard`], which returns
//! an [`Option`]: `None` when consent is absent, so the caller has *no value to emit*. There
//! is no API that produces a telemetry payload without first proving consent — the absence is
//! represented in the type system, not enforced by a runtime check the caller might forget.
//!
//! The consent *screen* itself is native chrome (D32) and is deferred; this is the seam the
//! screen will flip. Here we only model the boolean(s) and the gate.

use serde::{Deserialize, Serialize};

/// What a given player has (or hasn't) consented to. Distinct purposes are tracked
/// separately so the future consent screen can offer granular opt-in; today telemetry and
/// live-ops both read `analytics`. The store will read this same struct (a future
/// `purchases`/`marketing` field) — one authoritative representation, never two.
///
/// Default is **all-false**: absent or unknown consent is treated as *no consent*. A client
/// that never reached the consent screen, or whose state we can't parse, emits nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConsentState {
    /// Analytics / telemetry collection. Telemetry ingest and live-ops both gate on this.
    #[serde(default)]
    pub analytics: bool,
}

impl ConsentState {
    /// A fully-denying state — the safe default for any path that can't establish consent.
    pub const DENIED: ConsentState = ConsentState { analytics: false };

    /// Convenience constructor for a state that has opted into analytics.
    pub const fn analytics_granted() -> Self {
        ConsentState { analytics: true }
    }

    /// True only if analytics collection has been explicitly granted.
    pub const fn allows_analytics(self) -> bool {
        self.analytics
    }
}

/// The gate. Holds an authoritative [`ConsentState`] and is the *only* way to obtain a value
/// to emit. Constructing telemetry without going through `guard` is not possible from the
/// public API, which is what makes "no consent ⇒ no-op at the source" structural rather than
/// a discipline the caller has to remember.
#[derive(Debug, Clone, Copy)]
pub struct ConsentGate {
    state: ConsentState,
}

impl ConsentGate {
    /// Build a gate from an authoritative consent state (e.g. parsed from the request, or
    /// loaded from the accounts backend once it exists).
    pub const fn new(state: ConsentState) -> Self {
        ConsentGate { state }
    }

    /// A gate that denies everything — the default for unknown/unparseable consent.
    pub const fn denied() -> Self {
        ConsentGate::new(ConsentState::DENIED)
    }

    /// The authoritative state behind this gate.
    pub const fn state(self) -> ConsentState {
        self.state
    }

    /// **The consent-by-construction seam.** Pass the value you would emit; get it back only
    /// if analytics consent is present, otherwise `None`. Because the value is *moved in and
    /// only handed back on consent*, a no-consent caller is left holding nothing to send —
    /// the no-op happens at the source. Every emit path (telemetry, live-ops) funnels here.
    ///
    /// ```
    /// use gonedark_server::consent::{ConsentGate, ConsentState};
    /// let denied = ConsentGate::denied();
    /// assert_eq!(denied.guard(42), None);              // no consent ⇒ nothing to emit
    /// let granted = ConsentGate::new(ConsentState::analytics_granted());
    /// assert_eq!(granted.guard(42), Some(42));         // consent ⇒ value flows
    /// ```
    pub fn guard<T>(self, value: T) -> Option<T> {
        if self.state.allows_analytics() {
            Some(value)
        } else {
            None
        }
    }

    /// Lazy variant — the payload is only *constructed* when consent is present. Useful when
    /// building the event is itself non-trivial: under no consent we never even allocate it.
    pub fn guard_with<T, F: FnOnce() -> T>(self, build: F) -> Option<T> {
        if self.state.allows_analytics() {
            Some(build())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_denied() {
        assert_eq!(ConsentState::default(), ConsentState::DENIED);
        assert!(!ConsentState::default().allows_analytics());
    }

    #[test]
    fn denied_gate_guards_to_none() {
        // The central guarantee: no consent ⇒ no value to emit.
        let gate = ConsentGate::denied();
        assert_eq!(gate.guard("event"), None);
    }

    #[test]
    fn granted_gate_passes_value_through() {
        let gate = ConsentGate::new(ConsentState::analytics_granted());
        assert_eq!(gate.guard("event"), Some("event"));
    }

    #[test]
    fn guard_with_does_not_build_payload_without_consent() {
        use std::cell::Cell;
        let built = Cell::new(false);
        let gate = ConsentGate::denied();
        let out = gate.guard_with(|| {
            built.set(true);
            99
        });
        assert_eq!(out, None);
        assert!(!built.get(), "payload must NOT be constructed without consent");
    }

    #[test]
    fn guard_with_builds_payload_with_consent() {
        let gate = ConsentGate::new(ConsentState::analytics_granted());
        let out = gate.guard_with(|| 99);
        assert_eq!(out, Some(99));
    }
}
