use mpgs_domain::{CandidateAvailability, ModeFamily, RankingSignals, UserPreferences};
use serde::{Deserialize, Serialize};

use crate::unit;

/// Request-scoped constraints that are safe to enforce as hard filters. Stored
/// profile preferences remain soft unless the caller explicitly opts a field in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HardConstraints {
    pub modes: bool,
    pub party_size: bool,
    pub platforms: bool,
    pub languages: bool,
    pub session_length: bool,
    pub budget: bool,
}

impl HardConstraints {
    pub const NONE: Self = Self {
        modes: false,
        party_size: false,
        platforms: false,
        languages: false,
        session_length: false,
        budget: false,
    };

    pub const ALL: Self = Self {
        modes: true,
        party_size: true,
        platforms: true,
        languages: true,
        session_length: true,
        budget: true,
    };
}

impl Default for HardConstraints {
    fn default() -> Self {
        Self::NONE
    }
}

/// Apply objective playability filters before scoring. Long-term user profile
/// preferences are intentionally soft in this backward-compatible entry point.
pub fn hard_filter(
    prefs: &UserPreferences,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    dominant_mode: Option<&str>,
    signals: &RankingSignals,
    availability: &CandidateAvailability,
) -> bool {
    hard_filter_with_constraints(
        prefs,
        recommended_min,
        recommended_max,
        dominant_mode,
        signals,
        availability,
        &HardConstraints::NONE,
    )
}

/// Apply objective playability filters plus explicitly selected request-level
/// constraints. Returns false when the candidate must be dropped.
#[allow(clippy::too_many_arguments)]
pub fn hard_filter_with_constraints(
    prefs: &UserPreferences,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    dominant_mode: Option<&str>,
    signals: &RankingSignals,
    availability: &CandidateAvailability,
    constraints: &HardConstraints,
) -> bool {
    let mp = &signals.multiplayer;

    // A confirmed shutdown is objectively unplayable only when the game has no
    // known private-session or self-hosted path that can survive public service.
    if unit(mp.service_shutdown_risk) >= 1.0
        && (!signals.has_multiplayer_confidence
            || unit(signals.multiplayer_confidence.service_shutdown_risk) >= 0.5)
        && !known_positive(
            signals,
            mp.private_session,
            signals.multiplayer_confidence.private_session,
        )
        && !known_positive(
            signals,
            mp.self_host_or_dedicated,
            signals.multiplayer_confidence.self_host_or_dedicated,
        )
    {
        return false;
    }

    if constraints.modes
        && prefs
            .excluded_modes
            .iter()
            .any(|excluded| candidate_matches_excluded_mode(excluded, dominant_mode, signals))
    {
        return false;
    }

    // A known bound is enough to prove a mismatch; the other bound may remain unknown.
    let party = prefs.party_size;
    if constraints.party_size
        && (recommended_min.is_some_and(|min| party < min)
            || recommended_max.is_some_and(|max| party > max))
    {
        return false;
    }

    if (constraints.platforms && platform_list_mismatch(&prefs.platforms, &availability.platforms))
        || (constraints.languages && known_list_mismatch(&prefs.languages, &availability.languages))
    {
        return false;
    }

    if constraints.session_length
        && (availability
            .typical_session_minutes_min
            .is_some_and(|candidate_min| candidate_min > prefs.session_minutes_max)
            || availability
                .typical_session_minutes_max
                .is_some_and(|candidate_max| candidate_max < prefs.session_minutes_min))
    {
        return false;
    }

    if constraints.budget
        && availability.is_free != Some(true)
        && let (Some(max_price), Some(price), Some(currency)) = (
            prefs.budget_max_each_minor,
            availability.final_price_minor,
            availability.price_currency.as_deref(),
        )
        && currency.eq_ignore_ascii_case(&prefs.budget_currency)
        && price > max_price
    {
        return false;
    }

    true
}

fn candidate_matches_excluded_mode(
    excluded: &str,
    dominant_mode: Option<&str>,
    signals: &RankingSignals,
) -> bool {
    let family = ModeFamily::from_alias(excluded);
    let dominant_family = dominant_mode.map(ModeFamily::from_alias);
    if family != ModeFamily::Unknown && dominant_family == Some(family) {
        return true;
    }

    let mp = &signals.multiplayer;
    let confidence = &signals.multiplayer_confidence;
    match family {
        ModeFamily::PrivateCoop => {
            known_positive(signals, mp.online_coop, confidence.online_coop)
                || known_positive(signals, mp.private_session, confidence.private_session)
        }
        ModeFamily::SelfHosted => known_positive(
            signals,
            mp.self_host_or_dedicated,
            confidence.self_host_or_dedicated,
        ),
        ModeFamily::MatchmadePvp => {
            known_positive(signals, mp.matchmaking_core, confidence.matchmaking_core)
        }
        ModeFamily::PublicWorld => known_positive(
            signals,
            mp.public_world_dependency,
            confidence.public_world_dependency,
        ),
        ModeFamily::Mixed | ModeFamily::GenericMultiplayer => dominant_family == Some(family),
        ModeFamily::Unknown => dominant_mode.is_some_and(|mode| {
            mode.to_ascii_lowercase()
                .contains(&excluded.to_ascii_lowercase())
        }),
    }
}

fn known_positive(signals: &RankingSignals, value: f64, confidence: f64) -> bool {
    unit(value) >= 0.5 && (!signals.has_multiplayer_confidence || unit(confidence) >= 0.5)
}

fn known_list_mismatch(required: &[String], available: &[String]) -> bool {
    !required.is_empty()
        && !available.is_empty()
        && !required.iter().any(|required| {
            available
                .iter()
                .any(|available| required.eq_ignore_ascii_case(available))
        })
}

fn platform_list_mismatch(required: &[String], available: &[String]) -> bool {
    if required.is_empty() || available.is_empty() {
        return false;
    }
    !required.iter().any(|required| {
        // Steam's OS flags are not Deck compatibility evidence. Until a
        // Verified/Playable/Unsupported fact is present, a Deck request is
        // an unknown soft constraint and must not filter every game.
        if required.eq_ignore_ascii_case("steamdeck")
            && !available
                .iter()
                .any(|value| value.to_ascii_lowercase().starts_with("steamdeck_"))
        {
            return true;
        }
        available
            .iter()
            .any(|available| platform_value_matches(required, available))
    })
}

fn platform_value_matches(required: &str, available: &str) -> bool {
    required.eq_ignore_ascii_case(available)
        || (required.eq_ignore_ascii_case("steamdeck")
            && matches!(
                available.to_ascii_lowercase().as_str(),
                "steamdeck_verified" | "steamdeck_playable"
            ))
        || (matches!(required.to_ascii_lowercase().as_str(), "mac" | "macos")
            && matches!(available.to_ascii_lowercase().as_str(), "mac" | "macos"))
}

/// Mutate ranking signals with preference-derived personal_fit and group_size adjustments.
pub fn apply_personalization(
    prefs: &UserPreferences,
    signals: &mut RankingSignals,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    availability: &CandidateAvailability,
) {
    apply_personalization_with_constraints(
        prefs,
        signals,
        recommended_min,
        recommended_max,
        availability,
        &HardConstraints::NONE,
    );
}

/// Personalize with request-scoped certainty. Untouched onboarding defaults
/// shrink every preference-derived component to neutral, while an explicit
/// request constraint is trusted for its own dimension immediately.
pub fn apply_personalization_with_constraints(
    prefs: &UserPreferences,
    signals: &mut RankingSignals,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    availability: &CandidateAvailability,
    constraints: &HardConstraints,
) {
    let party = prefs.party_size;
    let (size_fit, size_mismatch) = match (recommended_min, recommended_max) {
        (Some(min), Some(max)) if party >= min && party <= max => (1.0, 0.0),
        (Some(min), Some(max)) => {
            let outside = if party < min {
                f64::from(min - party)
            } else {
                f64::from(party.saturating_sub(max))
            };
            (0.2, unit(outside / 8.0))
        }
        (Some(min), None) if party >= min => (0.75, 0.0),
        (Some(min), None) => (0.2, unit(f64::from(min - party) / 8.0)),
        (None, Some(max)) if party <= max => (0.75, 0.0),
        (None, Some(max)) => (0.2, unit(f64::from(party - max) / 8.0)),
        (None, None) => (0.5, 0.0),
    };
    signals.multiplayer.group_size_fit = size_fit;
    signals.multiplayer.group_size_mismatch = size_mismatch;

    let coop_pref = 1.0 - unit(prefs.coop_competitive);
    let competitive_pref = unit(prefs.coop_competitive);
    let host_pref = unit(prefs.self_hosting_willingness);

    let confidence = unit(signals.data_confidence);
    let mp_confidence = &signals.multiplayer_confidence;
    let capability_confidence = |value: f64| {
        if signals.has_multiplayer_confidence {
            unit(value)
        } else {
            confidence
        }
    };
    let group_confidence = match (recommended_min, recommended_max) {
        (Some(_), Some(_)) => capability_confidence(mp_confidence.group_size_fit),
        (Some(_), None) | (None, Some(_)) => {
            0.65 * capability_confidence(mp_confidence.group_size_fit)
        }
        (None, None) => 0.0,
    };
    let group_fit = confidence_weighted(size_fit, group_confidence);

    let coop_capability = (confidence_weighted(
        signals.multiplayer.online_coop,
        capability_confidence(mp_confidence.online_coop),
    ) + confidence_weighted(
        signals.multiplayer.private_session,
        capability_confidence(mp_confidence.private_session),
    )) / 2.0;
    let competitive_capability = confidence_weighted(
        signals.multiplayer.matchmaking_core,
        capability_confidence(mp_confidence.matchmaking_core),
    );
    let mode_fit = coop_pref * coop_capability + competitive_pref * competitive_capability;

    let access_observed = platform_fit(
        prefs,
        availability,
        signals.multiplayer.cross_platform_fit,
        capability_confidence(mp_confidence.cross_platform_fit),
    );
    let access_fit = confidence_weighted(access_observed.0, access_observed.1);

    let private_access = confidence_weighted(
        signals.multiplayer.private_session,
        capability_confidence(mp_confidence.private_session),
    );
    let low_public_dependency = confidence_weighted(
        signals.multiplayer.low_public_population_dependency,
        capability_confidence(mp_confidence.low_public_population_dependency),
    );
    let self_host_access = confidence_weighted(
        signals.multiplayer.self_host_or_dedicated,
        capability_confidence(mp_confidence.self_host_or_dedicated),
    );
    let managed_access = private_access.max(low_public_dependency);
    let hosted_access = managed_access.max(self_host_access);
    let hosting_fit = (1.0 - host_pref) * managed_access + host_pref * hosted_access;

    let session = session_fit(prefs, availability);
    let session_fit = confidence_weighted(session.0, session.1);
    let budget = budget_fit(prefs, availability);
    let budget_fit = confidence_weighted(budget.0, budget.1);
    let language = language_fit(prefs, availability);
    let language_fit = confidence_weighted(language.0, language.1);
    let stored_preference_confidence = unit(prefs.preference_confidence);
    let preference_fit = |fit: f64, explicit: bool| {
        let confidence = if explicit {
            1.0
        } else {
            stored_preference_confidence
        };
        confidence * unit(fit) + (1.0 - confidence) * 0.5
    };
    let group_fit = preference_fit(group_fit, constraints.party_size);
    let mode_fit = preference_fit(mode_fit, constraints.modes);
    let access_fit = preference_fit(access_fit, constraints.platforms);
    let hosting_fit = preference_fit(hosting_fit, constraints.modes);
    let session_fit = preference_fit(session_fit, constraints.session_length);
    let budget_fit = preference_fit(budget_fit, constraints.budget);
    let language_fit = preference_fit(language_fit, constraints.languages);

    signals.personal_components = mpgs_domain::PersonalFitSignals {
        group_fit,
        mode_fit,
        access_fit,
        hosting_fit,
        session_fit,
        budget_fit,
        language_fit,
    };

    signals.personal_fit = 0.25 * group_fit
        + 0.20 * mode_fit
        + 0.20 * access_fit
        + 0.15 * hosting_fit
        + 0.10 * session_fit
        + 0.07 * budget_fit
        + 0.03 * language_fit;
}

fn confidence_weighted(observed: f64, confidence: f64) -> f64 {
    let confidence = unit(confidence);
    confidence * unit(observed) + (1.0 - confidence) * 0.5
}

/// Returns (fit, fact-known marker). Platform availability and cross-play are
/// distinct facts: storefront platform support must not overwrite cross-play.
fn platform_fit(
    prefs: &UserPreferences,
    availability: &CandidateAvailability,
    cross_platform_fit: f64,
    multiplayer_confidence: f64,
) -> (f64, f64) {
    if prefs.platforms.is_empty() || availability.platforms.is_empty() {
        return (0.5, 0.0);
    }
    let requirement_fits = prefs
        .platforms
        .iter()
        .map(|required| platform_requirement_fit(required, &availability.platforms))
        .collect::<Vec<_>>();
    let store_support =
        requirement_fits.iter().map(|(fit, _)| fit).sum::<f64>() / requirement_fits.len() as f64;
    let store_confidence = requirement_fits
        .iter()
        .map(|(_, confidence)| confidence)
        .sum::<f64>()
        / requirement_fits.len() as f64;

    if prefs.platforms.len() > 1 {
        let crossplay = confidence_weighted(cross_platform_fit, multiplayer_confidence);
        (
            (store_support + crossplay) / 2.0,
            (store_confidence + unit(multiplayer_confidence)) / 2.0,
        )
    } else {
        (store_support, store_confidence)
    }
}

fn platform_requirement_fit(required: &str, available: &[String]) -> (f64, f64) {
    if required.eq_ignore_ascii_case("steamdeck") {
        // Store OS flags are not Deck compatibility observations. Only the
        // dedicated compatibility state is allowed to move fit away from its
        // neutral prior.
        if available
            .iter()
            .any(|value| value.eq_ignore_ascii_case("steamdeck_unsupported"))
        {
            return (0.0, 1.0);
        }
        if available
            .iter()
            .any(|value| value.eq_ignore_ascii_case("steamdeck_verified"))
        {
            return (1.0, 1.0);
        }
        if available
            .iter()
            .any(|value| value.eq_ignore_ascii_case("steamdeck_playable"))
        {
            return (0.8, 1.0);
        }
        return (0.5, 0.0);
    }

    (
        f64::from(
            available
                .iter()
                .any(|available| platform_value_matches(required, available)),
        ),
        1.0,
    )
}

fn session_fit(prefs: &UserPreferences, availability: &CandidateAvailability) -> (f64, f64) {
    let bounds = (
        availability.typical_session_minutes_min,
        availability.typical_session_minutes_max,
    );
    let (Some(candidate_min), Some(candidate_max)) = bounds else {
        return match bounds {
            (Some(candidate_min), None) if candidate_min <= prefs.session_minutes_max => {
                (0.75, 0.65)
            }
            (None, Some(candidate_max)) if candidate_max >= prefs.session_minutes_min => {
                (0.75, 0.65)
            }
            (Some(_), None) | (None, Some(_)) => (0.0, 1.0),
            (None, None) => (0.5, 0.0),
            (Some(_), Some(_)) => unreachable!(),
        };
    };
    let overlap_min = candidate_min.max(prefs.session_minutes_min);
    let overlap_max = candidate_max.min(prefs.session_minutes_max);
    if overlap_min > overlap_max {
        return (0.0, 1.0);
    }
    let candidate_span = candidate_max.saturating_sub(candidate_min);
    if candidate_span == 0 {
        return (1.0, 1.0);
    }
    (
        unit(f64::from(overlap_max - overlap_min) / f64::from(candidate_span)),
        1.0,
    )
}

fn budget_fit(prefs: &UserPreferences, availability: &CandidateAvailability) -> (f64, f64) {
    if availability.is_free == Some(true) {
        return (1.0, 1.0);
    }
    let (Some(max_price), Some(price), Some(currency)) = (
        prefs.budget_max_each_minor,
        availability.final_price_minor,
        availability.price_currency.as_deref(),
    ) else {
        return (0.5, 0.0);
    };
    if !currency.eq_ignore_ascii_case(&prefs.budget_currency) {
        return (0.5, 0.0);
    }
    if max_price == 0 {
        return (f64::from(price == 0), 1.0);
    }
    let price_ratio = price.max(0) as f64 / max_price as f64;
    (unit(1.0 - 0.5 * price_ratio), 1.0)
}

fn language_fit(prefs: &UserPreferences, availability: &CandidateAvailability) -> (f64, f64) {
    if prefs.languages.is_empty() || availability.languages.is_empty() {
        return (0.5, 0.0);
    }
    (
        f64::from(!known_list_mismatch(
            &prefs.languages,
            &availability.languages,
        )),
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        HardConstraints, apply_personalization, apply_personalization_with_constraints,
        hard_filter_with_constraints,
    };
    use mpgs_domain::{CandidateAvailability, MultiplayerSignals, RankingSignals, UserPreferences};

    fn hard_filter(
        prefs: &UserPreferences,
        recommended_min: Option<u8>,
        recommended_max: Option<u8>,
        dominant_mode: Option<&str>,
        signals: &RankingSignals,
        availability: &CandidateAvailability,
    ) -> bool {
        hard_filter_with_constraints(
            prefs,
            recommended_min,
            recommended_max,
            dominant_mode,
            signals,
            availability,
            &HardConstraints::ALL,
        )
    }

    fn multiplayer_candidate() -> RankingSignals {
        RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 1.0,
                ..Default::default()
            },
            data_confidence: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn one_sided_player_bounds_filter_known_mismatches() {
        let availability = CandidateAvailability::default();
        let signals = multiplayer_candidate();

        let one_player = UserPreferences {
            party_size: 1,
            ..UserPreferences::default()
        };
        assert!(!hard_filter(
            &one_player,
            Some(2),
            None,
            None,
            &signals,
            &availability,
        ));

        let five_players = UserPreferences {
            party_size: 5,
            ..UserPreferences::default()
        };
        assert!(!hard_filter(
            &five_players,
            None,
            Some(4),
            None,
            &signals,
            &availability,
        ));

        let four_players = UserPreferences::default();
        assert!(hard_filter(
            &four_players,
            Some(2),
            None,
            None,
            &signals,
            &availability,
        ));
        assert!(hard_filter(
            &four_players,
            None,
            Some(4),
            None,
            &signals,
            &availability,
        ));
    }

    #[test]
    fn stored_preferences_are_soft_without_request_constraints() {
        let prefs = UserPreferences {
            party_size: 1,
            excluded_modes: vec!["pvp".into()],
            platforms: vec!["windows".into()],
            ..UserPreferences::default()
        };
        let signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                matchmaking_core: 1.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let availability = CandidateAvailability {
            platforms: vec!["linux".into()],
            ..Default::default()
        };

        assert!(super::hard_filter(
            &prefs,
            Some(2),
            Some(4),
            Some("matchmade_pvp"),
            &signals,
            &availability,
        ));
    }

    #[test]
    fn confirmed_shutdown_requires_a_known_surviving_private_path() {
        let prefs = UserPreferences::default();
        let availability = CandidateAvailability::default();
        let mut signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 0.5,
                self_host_or_dedicated: 0.5,
                service_shutdown_risk: 1.0,
                ..Default::default()
            },
            multiplayer_confidence: MultiplayerSignals {
                service_shutdown_risk: 1.0,
                ..Default::default()
            },
            has_multiplayer_confidence: true,
            ..Default::default()
        };
        assert!(!super::hard_filter(
            &prefs,
            None,
            None,
            None,
            &signals,
            &availability,
        ));

        signals.multiplayer.self_host_or_dedicated = 1.0;
        signals.multiplayer_confidence.self_host_or_dedicated = 0.9;
        assert!(super::hard_filter(
            &prefs,
            None,
            None,
            None,
            &signals,
            &availability,
        ));
    }

    #[test]
    fn excluded_mode_aliases_use_canonical_mode_family() {
        let signals = multiplayer_candidate();
        let availability = CandidateAvailability::default();
        let prefs = UserPreferences {
            excluded_modes: vec!["pvp_only".into()],
            ..UserPreferences::default()
        };

        assert!(!hard_filter(
            &prefs,
            Some(2),
            Some(8),
            Some("matchmade_pvp"),
            &signals,
            &availability,
        ));
        assert!(!hard_filter(
            &prefs,
            Some(2),
            Some(8),
            Some("competitive"),
            &signals,
            &availability,
        ));
    }

    #[test]
    fn steam_deck_without_compatibility_evidence_is_soft_unknown() {
        let prefs = UserPreferences {
            platforms: vec!["steamdeck".into()],
            ..UserPreferences::default()
        };
        let signals = multiplayer_candidate();
        let os_only = CandidateAvailability {
            platforms: vec!["windows".into(), "linux".into()],
            ..Default::default()
        };
        assert!(hard_filter(
            &prefs,
            Some(1),
            Some(4),
            None,
            &signals,
            &os_only,
        ));

        let unsupported = CandidateAvailability {
            platforms: vec!["steamdeck_unsupported".into()],
            ..Default::default()
        };
        assert!(!hard_filter(
            &prefs,
            Some(1),
            Some(4),
            None,
            &signals,
            &unsupported,
        ));
    }

    #[test]
    fn one_sided_session_bounds_filter_proven_mismatches() {
        let prefs = UserPreferences {
            session_minutes_min: 30,
            session_minutes_max: 180,
            ..UserPreferences::default()
        };
        let signals = multiplayer_candidate();

        let too_long = CandidateAvailability {
            typical_session_minutes_min: Some(240),
            ..Default::default()
        };
        assert!(!hard_filter(
            &prefs,
            Some(1),
            Some(4),
            None,
            &signals,
            &too_long,
        ));

        let too_short = CandidateAvailability {
            typical_session_minutes_max: Some(15),
            ..Default::default()
        };
        assert!(!hard_filter(
            &prefs,
            Some(1),
            Some(4),
            None,
            &signals,
            &too_short,
        ));
    }

    #[test]
    fn one_sided_player_bounds_contribute_partial_fit() {
        let availability = CandidateAvailability::default();
        let prefs = UserPreferences {
            party_size: 4,
            ..UserPreferences::default()
        };

        let mut min_only = multiplayer_candidate();
        apply_personalization(&prefs, &mut min_only, Some(2), None, &availability);
        assert_eq!(min_only.multiplayer.group_size_fit, 0.75);
        assert_eq!(min_only.multiplayer.group_size_mismatch, 0.0);

        let mut max_only = multiplayer_candidate();
        apply_personalization(&prefs, &mut max_only, None, Some(4), &availability);
        assert_eq!(max_only.multiplayer.group_size_fit, 0.75);
        assert_eq!(max_only.multiplayer.group_size_mismatch, 0.0);
    }

    #[test]
    fn player_range_midpoint_does_not_overflow_u8() {
        let availability = CandidateAvailability::default();
        let prefs = UserPreferences {
            party_size: 2,
            ..UserPreferences::default()
        };
        let mut signals = multiplayer_candidate();

        apply_personalization(&prefs, &mut signals, Some(2), Some(u8::MAX), &availability);

        assert_eq!(signals.multiplayer.group_size_fit, 1.0);
        assert!(signals.multiplayer.group_size_fit.is_finite());
    }

    #[test]
    fn low_confidence_evidence_is_shrunk_toward_neutral_instead_of_saturating() {
        let prefs = UserPreferences::default();
        let availability = CandidateAvailability {
            platforms: vec!["windows".into()],
            languages: vec!["schinese".into()],
            typical_session_minutes_min: Some(45),
            typical_session_minutes_max: Some(90),
            price_currency: Some("CNY".into()),
            final_price_minor: Some(5_000),
            is_free: Some(false),
        };
        let mut signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 1.0,
                self_host_or_dedicated: 1.0,
                online_coop: 1.0,
                low_public_population_dependency: 1.0,
                drop_in_out: 1.0,
                cross_platform_fit: 1.0,
                ..Default::default()
            },
            data_confidence: 0.3,
            ..Default::default()
        };

        apply_personalization(&prefs, &mut signals, Some(1), Some(4), &availability);

        assert!(
            (0.5..0.8).contains(&signals.personal_fit),
            "low-confidence positive evidence must not saturate: {}",
            signals.personal_fit
        );
    }

    #[test]
    fn unknown_personalization_facts_remain_neutral() {
        let prefs = UserPreferences::default();
        let mut signals = RankingSignals::default();

        apply_personalization(
            &prefs,
            &mut signals,
            None,
            None,
            &CandidateAvailability::default(),
        );

        assert!((signals.personal_fit - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn rich_catalog_data_does_not_turn_unknown_capabilities_into_negative_facts() {
        let prefs = UserPreferences::default();
        let mut signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 0.5,
                self_host_or_dedicated: 0.5,
                online_coop: 0.5,
                low_public_population_dependency: 0.5,
                drop_in_out: 0.5,
                cross_platform_fit: 0.5,
                ..Default::default()
            },
            multiplayer_confidence: MultiplayerSignals::default(),
            has_multiplayer_confidence: true,
            data_confidence: 0.9,
            ..Default::default()
        };

        apply_personalization(
            &prefs,
            &mut signals,
            None,
            None,
            &CandidateAvailability::default(),
        );

        assert!((signals.personal_fit - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn platform_availability_does_not_fabricate_crossplay_evidence() {
        let prefs = UserPreferences {
            platforms: vec!["windows".into(), "linux".into()],
            ..UserPreferences::default()
        };
        let availability = CandidateAvailability {
            platforms: vec!["windows".into(), "linux".into()],
            ..Default::default()
        };
        let mut signals = multiplayer_candidate();
        signals.multiplayer.cross_platform_fit = 0.2;

        apply_personalization(&prefs, &mut signals, Some(1), Some(4), &availability);

        assert_eq!(signals.multiplayer.cross_platform_fit, 0.2);
    }

    #[test]
    fn mode_preference_changes_direction_without_component_saturation() {
        let availability = CandidateAvailability::default();
        let mut coop = RankingSignals {
            multiplayer: MultiplayerSignals {
                online_coop: 1.0,
                private_session: 1.0,
                low_public_population_dependency: 1.0,
                ..Default::default()
            },
            data_confidence: 1.0,
            ..Default::default()
        };
        let mut pvp = RankingSignals {
            multiplayer: MultiplayerSignals {
                matchmaking_core: 1.0,
                low_public_population_dependency: 1.0,
                ..Default::default()
            },
            data_confidence: 1.0,
            ..Default::default()
        };

        let coop_prefs = UserPreferences {
            preference_confidence: 1.0,
            coop_competitive: 0.0,
            self_hosting_willingness: 0.0,
            ..UserPreferences::default()
        };
        apply_personalization(&coop_prefs, &mut coop, Some(1), Some(4), &availability);
        apply_personalization(&coop_prefs, &mut pvp, Some(1), Some(4), &availability);
        assert!(coop.personal_fit > pvp.personal_fit);

        let competitive_prefs = UserPreferences {
            preference_confidence: 1.0,
            coop_competitive: 1.0,
            self_hosting_willingness: 0.0,
            ..UserPreferences::default()
        };
        apply_personalization(
            &competitive_prefs,
            &mut coop,
            Some(1),
            Some(4),
            &availability,
        );
        apply_personalization(
            &competitive_prefs,
            &mut pvp,
            Some(1),
            Some(4),
            &availability,
        );
        assert!(pvp.personal_fit > coop.personal_fit);
    }

    #[test]
    fn untouched_onboarding_defaults_are_neutral_until_confirmed_or_explicit() {
        let prefs = UserPreferences::default();
        let availability = CandidateAvailability::default();
        let mut signals = multiplayer_candidate();
        apply_personalization(&prefs, &mut signals, Some(2), Some(4), &availability);
        assert!((signals.personal_fit - 0.5).abs() < 1e-12);
        assert_eq!(signals.personal_components.group_fit, 0.5);

        let explicit_party = HardConstraints {
            party_size: true,
            ..HardConstraints::NONE
        };
        apply_personalization_with_constraints(
            &prefs,
            &mut signals,
            Some(2),
            Some(4),
            &availability,
            &explicit_party,
        );
        assert!(signals.personal_components.group_fit > 0.5);
        assert_eq!(signals.personal_components.mode_fit, 0.5);
    }
}
