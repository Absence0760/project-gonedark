//! Live-ops scaffolding — the remote-tunable config/flag surface live-ops will need. Phase 4
//! WS-D step 3 (docs/plans/phase-4-plan.md §4). Consistent with the data/config hot-reload lean in
//! roadmap.md: values live as data the server hands out, not compiled-in constants.
//!
//! **Scaffold only — no live-ops *content* this phase.** This is the *shape* of the surface:
//! a typed config snapshot the client fetches, plus the consent gate applied the *same* way as
//! telemetry. Live-ops config is split into two tiers by their consent relationship:
//!
//! - **Public config** is fairness/operational (e.g. a server-driven message, a maintenance
//!   flag) and is returned to *everyone* — withholding it can't break consent, and a client
//!   needs it before any consent decision.
//! - **Personalized config** (A/B buckets, targeted experiments) is analytics-derived and is
//!   therefore consent-gated: a non-consenting client gets `None` — the same "no-op at the
//!   source" rule as telemetry, routed through the same [`ConsentGate`].
//!
//! Server-side only; no `core`/`engine` deps, no determinism concern.

use serde::{Deserialize, Serialize};

use crate::consent::ConsentGate;

/// Operational config returned to *every* client regardless of consent. Nothing here is
/// derived from analytics, so it's safe (and necessary) pre-consent. Scaffold fields only.
///
/// The derived `Default` (no maintenance, no minimum build) is the committed, non-secret
/// clone-and-run baseline (invariant #8) — no real config values are compiled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PublicConfig {
    /// Soft maintenance flag — when true the shell shows a "service unavailable" notice.
    pub maintenance: bool,
    /// Minimum supported client build; older clients should prompt to update. `0` = none.
    pub min_supported_build: u32,
}

/// Analytics-derived, per-player config. Only delivered to consenting clients. Scaffold:
/// a single A/B bucket placeholder; real experiments slot in here later. The derived
/// `Default` is the unbucketed baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersonalizedConfig {
    /// Experiment bucket assignment (e.g. `"control"`). Empty = unbucketed.
    pub experiment_bucket: String,
}

/// The full config response. `personalized` is `None` for non-consenting clients — the
/// consent-by-construction no-op applied to live-ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveOpsConfig {
    pub public: PublicConfig,
    /// Present only when analytics consent was granted (see [`resolve`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personalized: Option<PersonalizedConfig>,
}

/// The live-ops source. In a real deployment this reads tunables from Redis/Postgres
/// (docs/infrastructure.md) with hot-reload; the scaffold holds them in memory. Kept behind a
/// struct so the resolve logic is testable without a backing store.
#[derive(Debug, Clone, Default)]
pub struct LiveOpsSource {
    public: PublicConfig,
    personalized: PersonalizedConfig,
}

impl LiveOpsSource {
    /// A source seeded with the committed non-secret defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the public config (e.g. flip maintenance on). Scaffold setter.
    pub fn with_public(mut self, public: PublicConfig) -> Self {
        self.public = public;
        self
    }

    /// Override the personalized config the consenting tier receives. Scaffold setter.
    pub fn with_personalized(mut self, personalized: PersonalizedConfig) -> Self {
        self.personalized = personalized;
        self
    }

    /// **The single consent-gated resolve path.** Public config always returns; personalized
    /// config is passed through [`ConsentGate::guard`] so a non-consenting client gets `None`
    /// — same structural no-op as telemetry, same gate.
    pub fn resolve(&self, gate: ConsentGate) -> LiveOpsConfig {
        LiveOpsConfig {
            public: self.public.clone(),
            personalized: gate.guard(self.personalized.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::ConsentState;

    #[test]
    fn public_config_returned_without_consent() {
        let src = LiveOpsSource::new().with_public(PublicConfig {
            maintenance: true,
            min_supported_build: 42,
        });
        let cfg = src.resolve(ConsentGate::denied());
        assert!(cfg.public.maintenance);
        assert_eq!(cfg.public.min_supported_build, 42);
    }

    #[test]
    fn personalized_config_withheld_without_consent() {
        // The consent-by-construction rule applied to live-ops.
        let src = LiveOpsSource::new().with_personalized(PersonalizedConfig {
            experiment_bucket: "treatment".into(),
        });
        let cfg = src.resolve(ConsentGate::denied());
        assert_eq!(cfg.personalized, None, "no consent ⇒ no personalized config");
    }

    #[test]
    fn personalized_config_delivered_with_consent() {
        let src = LiveOpsSource::new().with_personalized(PersonalizedConfig {
            experiment_bucket: "treatment".into(),
        });
        let cfg = src.resolve(ConsentGate::new(ConsentState::analytics_granted()));
        assert_eq!(
            cfg.personalized,
            Some(PersonalizedConfig {
                experiment_bucket: "treatment".into()
            })
        );
    }

    #[test]
    fn no_consent_response_omits_personalized_in_json() {
        let cfg = LiveOpsSource::new().resolve(ConsentGate::denied());
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(!s.contains("personalized"), "field skipped when None: {s}");
    }
}
