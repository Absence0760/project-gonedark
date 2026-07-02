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
//!
//! ## CP-8 — the rotating scenario-modifier bridge
//!
//! [`PublicConfig::modifier_rotation_period`]/[`PublicConfig::modifier_track`] and
//! [`PersonalizedConfig::modifier_track_override`] are the *entire* live-ops surface for the PvE
//! WS-E rotating-modifier system (`docs/roadmap.md` CP-8). They are deliberately just two plain
//! scalars (a `u64` period, a `u32` track index) — never a `ScenarioModifiers` value, never a
//! balance number. Turning that pair into real scenario tuning is `core`'s job alone
//! (`gonedark_core::mission_tuning::ScenarioModifiers::for_rotation`), reached from the client
//! host (`engine`/`app`), never here — this crate stays `core`/`engine`-free by design (see the
//! module docs above), so it is structurally impossible for a live-ops payload issued from here to
//! *contain* a modifier, let alone a balance-touching one. It can only ever name an index.
//!
//! The **period** stays on [`PublicConfig`]: "this week's rotation is on" is the same fact for
//! every player, not an analytics derivation, so withholding it pre-consent would just break the
//! feature for everyone (same reasoning as `maintenance`). The **track override** lives on
//! [`PersonalizedConfig`]: which catalog a given player's rotation pulls from *can* be an
//! analytics-derived A/B cohort assignment, so it is consent-gated exactly like
//! `experiment_bucket` — [`LiveOpsConfig::effective_modifier_track`] is the one seam that resolves
//! "public baseline vs. personalized override" into the track a client should actually use.

use serde::{Deserialize, Serialize};

use crate::consent::ConsentGate;

/// Operational config returned to *every* client regardless of consent. Nothing here is
/// derived from analytics, so it's safe (and necessary) pre-consent. Scaffold fields only.
///
/// The derived `Default` (no maintenance, no minimum build, no rotation active, standard track)
/// is the committed, non-secret clone-and-run baseline (invariant #8) — no real config values are
/// compiled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PublicConfig {
    /// Soft maintenance flag — when true the shell shows a "service unavailable" notice.
    pub maintenance: bool,
    /// Minimum supported client build; older clients should prompt to update. `0` = none.
    pub min_supported_build: u32,
    /// CP-8: the live-ops rotation period currently active, or `None` when no rotation is
    /// scheduled. `None` is the default — a client with no live-ops override runs a mission's own
    /// authored `ScenarioModifiers`, unperturbed (mirrors `ScenarioModifiers::reinforcement_period`'s
    /// own "`None` ⇒ no-op" shape). A client resolves `Some(period)` via
    /// `gonedark_core::mission_tuning::ScenarioModifiers::for_rotation(period, track)`.
    pub modifier_rotation_period: Option<u64>,
    /// CP-8: the rotation **track** baseline every client gets absent a personalized override — a
    /// plain wire index (`0` = the standard catalog; see `core::mission_tuning`). Never
    /// analytics-derived, so it is safe (and necessary) pre-consent.
    pub modifier_track: u32,
}

/// Analytics-derived, per-player config. Only delivered to consenting clients. Scaffold:
/// a single A/B bucket placeholder plus the CP-8 rotation-track override; real experiments slot
/// in here later. The derived `Default` is the unbucketed baseline (no override).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersonalizedConfig {
    /// Experiment bucket assignment (e.g. `"control"`). Empty = unbucketed.
    pub experiment_bucket: String,
    /// CP-8: an analytics-derived override of [`PublicConfig::modifier_track`] — e.g. this
    /// player's A/B cohort runs a different rotation catalog than the public baseline. `None` =
    /// no override (use the public track). Consent-gated like every other field on this struct;
    /// still just a plain wire index, never a modifier value.
    pub modifier_track_override: Option<u32>,
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

impl LiveOpsConfig {
    /// CP-8: the modifier track a client should actually resolve its rotation through — the one
    /// seam that folds consent into "which catalog". Returns the personalized override *only* when
    /// it is both present (consent was granted, see [`LiveOpsSource::resolve`]) and itself set
    /// (`Some`); otherwise falls back to [`PublicConfig::modifier_track`]. A non-consenting client
    /// (`personalized: None`) always lands on the public baseline — it can never see, let alone
    /// run, an experiment-assigned track.
    pub fn effective_modifier_track(&self) -> u32 {
        self.personalized
            .as_ref()
            .and_then(|p| p.modifier_track_override)
            .unwrap_or(self.public.modifier_track)
    }
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
            ..PublicConfig::default()
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
            ..PersonalizedConfig::default()
        });
        let cfg = src.resolve(ConsentGate::denied());
        assert_eq!(cfg.personalized, None, "no consent ⇒ no personalized config");
    }

    #[test]
    fn personalized_config_delivered_with_consent() {
        let src = LiveOpsSource::new().with_personalized(PersonalizedConfig {
            experiment_bucket: "treatment".into(),
            ..PersonalizedConfig::default()
        });
        let cfg = src.resolve(ConsentGate::new(ConsentState::analytics_granted()));
        assert_eq!(
            cfg.personalized,
            Some(PersonalizedConfig {
                experiment_bucket: "treatment".into(),
                ..PersonalizedConfig::default()
            })
        );
    }

    #[test]
    fn no_consent_response_omits_personalized_in_json() {
        let cfg = LiveOpsSource::new().resolve(ConsentGate::denied());
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(!s.contains("personalized"), "field skipped when None: {s}");
    }

    // --- CP-8: the rotation descriptor + consent-gated track override ---------------------------

    /// Default config carries no active rotation and the standard (`0`) track — the committed,
    /// non-secret clone-and-run baseline (invariant #8): a fresh checkout's live-ops config
    /// perturbs no scenario.
    #[test]
    fn default_config_has_no_active_rotation() {
        let cfg = LiveOpsSource::new().resolve(ConsentGate::denied());
        assert_eq!(cfg.public.modifier_rotation_period, None);
        assert_eq!(cfg.public.modifier_track, 0);
        assert_eq!(cfg.effective_modifier_track(), 0);
    }

    /// The rotation period is public — it is returned even without consent, same as
    /// `maintenance`/`min_supported_build`.
    #[test]
    fn rotation_period_is_returned_without_consent() {
        let src = LiveOpsSource::new().with_public(PublicConfig {
            modifier_rotation_period: Some(7),
            modifier_track: 0,
            ..PublicConfig::default()
        });
        let cfg = src.resolve(ConsentGate::denied());
        assert_eq!(cfg.public.modifier_rotation_period, Some(7));
    }

    /// Without consent, a personalized track override never applies — `effective_modifier_track`
    /// must fall back to the public baseline. This is the CP-8 instance of the same
    /// consent-by-construction guarantee `personalized_config_withheld_without_consent` proves for
    /// the whole struct.
    #[test]
    fn track_override_is_not_applied_without_consent() {
        let src = LiveOpsSource::new()
            .with_public(PublicConfig {
                modifier_track: 0,
                ..PublicConfig::default()
            })
            .with_personalized(PersonalizedConfig {
                modifier_track_override: Some(1),
                ..PersonalizedConfig::default()
            });
        let cfg = src.resolve(ConsentGate::denied());
        assert_eq!(cfg.personalized, None, "no consent ⇒ no personalized config at all");
        assert_eq!(
            cfg.effective_modifier_track(),
            0,
            "no consent ⇒ the public baseline track, never the experiment override"
        );
    }

    /// With consent, a set track override wins over the public baseline.
    #[test]
    fn track_override_applies_with_consent() {
        let src = LiveOpsSource::new()
            .with_public(PublicConfig {
                modifier_track: 0,
                ..PublicConfig::default()
            })
            .with_personalized(PersonalizedConfig {
                modifier_track_override: Some(1),
                ..PersonalizedConfig::default()
            });
        let cfg = src.resolve(ConsentGate::new(ConsentState::analytics_granted()));
        assert_eq!(cfg.effective_modifier_track(), 1);
    }

    /// With consent but no override set (`None`), the effective track still falls back to public —
    /// consent alone doesn't invent an experiment assignment.
    #[test]
    fn no_override_set_falls_back_to_public_track_even_with_consent() {
        let src = LiveOpsSource::new().with_public(PublicConfig {
            modifier_track: 3,
            ..PublicConfig::default()
        });
        let cfg = src.resolve(ConsentGate::new(ConsentState::analytics_granted()));
        assert_eq!(cfg.personalized, Some(PersonalizedConfig::default()));
        assert_eq!(cfg.effective_modifier_track(), 3);
    }
}
