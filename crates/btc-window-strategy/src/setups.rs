//! Window-specific entry setups. Multiple signals collapse into one candidate.

use crate::config::WindowConfig;
use crate::features::{side_ask, side_spread, SecondFeatures};
use crate::policy::{ActivePolicy, SetupGateParams};
use crate::pricing::{
    ask_policy_rejects, calibrated_probability, compute_pricing, edge_passes, price_band_ok,
    side_reference_agrees, PricingView,
};
use crate::types::{
    ConfidenceLabel, MarketSecondSnapshot, OutcomeSide, PerformanceBook, WindowKind,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryCandidate {
    pub setup_id: String,
    pub side: OutcomeSide,
    pub pricing: PricingView,
    pub confidence_label: ConfidenceLabel,
    pub book: PerformanceBook,
    pub experimental: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupReject {
    pub setup_id: &'static str,
    pub reason: &'static str,
}

#[must_use]
pub fn evaluate_window_candidates(
    snapshot: &MarketSecondSnapshot,
    features: &SecondFeatures,
    cfg: &WindowConfig,
    checkpoint_favourite: Option<OutcomeSide>,
    policy: &ActivePolicy,
) -> (Option<EntryCandidate>, Vec<SetupReject>) {
    let mut rejects = Vec::new();
    let mut best: Option<EntryCandidate> = None;
    let setup_ids: &[&'static str] = match snapshot.window {
        WindowKind::FiveMinute => &[
            "5m_confirmed_mid_window_favourite",
            "5m_high_confidence_confirmation",
            "5m_late_continuation",
        ],
        WindowKind::FifteenMinute => &[
            "15m_four_minute_persistent_favourite",
            "15m_seven_minute_acceleration",
            "15m_eleven_minute_continuation",
        ],
        WindowKind::OneHour => &[
            "1h_minute50_confirmed_favourite",
            "1h_experimental_sequential_hedge_seed",
        ],
    };
    for setup_id in setup_ids {
        consider(
            &mut best,
            &mut rejects,
            eval_gated(
                setup_id,
                snapshot,
                features,
                cfg,
                checkpoint_favourite,
                policy,
            ),
        );
    }
    (best, rejects)
}

fn consider(
    best: &mut Option<EntryCandidate>,
    rejects: &mut Vec<SetupReject>,
    result: Result<EntryCandidate, SetupReject>,
) {
    match result {
        Ok(candidate) => {
            let replace = match best.as_ref() {
                None => true,
                Some(prev) => candidate.pricing.edge_micros > prev.pricing.edge_micros,
            };
            if replace {
                *best = Some(candidate);
            }
        }
        Err(reject) => rejects.push(reject),
    }
}

fn eval_gated(
    setup_id: &'static str,
    snapshot: &MarketSecondSnapshot,
    features: &SecondFeatures,
    cfg: &WindowConfig,
    checkpoint_favourite: Option<OutcomeSide>,
    policy: &ActivePolicy,
) -> Result<EntryCandidate, SetupReject> {
    let params = policy.setup(setup_id).ok_or(SetupReject {
        setup_id,
        reason: "SETUP_UNKNOWN",
    })?;
    apply_setup_gates(
        setup_id,
        snapshot,
        features,
        cfg,
        checkpoint_favourite,
        params,
    )
}

fn in_window(elapsed: i64, lo: i64, hi: i64) -> bool {
    (lo..=hi).contains(&elapsed)
}

fn apply_setup_gates(
    setup_id: &'static str,
    snapshot: &MarketSecondSnapshot,
    features: &SecondFeatures,
    cfg: &WindowConfig,
    checkpoint_favourite: Option<OutcomeSide>,
    params: &SetupGateParams,
) -> Result<EntryCandidate, SetupReject> {
    if !params.enabled {
        return Err(SetupReject {
            setup_id,
            reason: "SETUP_DISABLED",
        });
    }
    let min_confidence = resolve_entry_confidence(setup_id, snapshot.elapsed_secs, params)?;
    reject_persistence(setup_id, snapshot, features, params)?;
    reject_checkpoint(setup_id, features.favourite, checkpoint_favourite, params)?;
    reject_momentum_and_signal(setup_id, features, params)?;

    let mut candidate = base_side_checks(SideCheckParams {
        setup_id,
        snapshot,
        features,
        cfg,
        side: features.favourite,
        min_confidence,
        max_ask_override: if params.max_ask_override_micros > 0 {
            Some(params.max_ask_override_micros)
        } else {
            None
        },
        require_non_negative_mom: params.require_non_negative_mom,
    })?;
    maybe_demote_shadow(setup_id, params, &mut candidate);
    Ok(candidate)
}

fn resolve_entry_confidence(
    setup_id: &'static str,
    elapsed: i64,
    params: &SetupGateParams,
) -> Result<i64, SetupReject> {
    let primary = in_window(
        elapsed,
        params.entry_elapsed_lo_secs,
        params.entry_elapsed_hi_secs,
    );
    let secondary = params.entry_elapsed_lo2_secs != 0
        && params.entry_elapsed_hi2_secs != 0
        && in_window(
            elapsed,
            params.entry_elapsed_lo2_secs,
            params.entry_elapsed_hi2_secs,
        );
    if !primary && !secondary {
        return Err(SetupReject {
            setup_id,
            reason: "OUTSIDE_ENTRY_WINDOW",
        });
    }
    if secondary && !primary && params.min_confidence_window2_micros > 0 {
        Ok(params.min_confidence_window2_micros)
    } else {
        Ok(params.min_confidence_micros)
    }
}

fn reject_persistence(
    setup_id: &'static str,
    snapshot: &MarketSecondSnapshot,
    features: &SecondFeatures,
    params: &SetupGateParams,
) -> Result<(), SetupReject> {
    if params.min_persistence_secs > 0 && features.persistence_secs < params.min_persistence_secs {
        return Err(SetupReject {
            setup_id,
            reason: if setup_id.contains("persistent") {
                "RECENT_FLIP"
            } else {
                "PERSISTENCE_TOO_SHORT"
            },
        });
    }
    if params.persistence_since_offset_secs > 0 {
        let need = snapshot
            .elapsed_secs
            .saturating_sub(params.persistence_since_offset_secs);
        if features.persistence_secs < need {
            let reason = match params.persistence_since_offset_secs {
                120 => "PERSISTENCE_SINCE_120_REQUIRED",
                600 => "PERSISTENCE_SINCE_600_REQUIRED",
                _ => "PERSISTENCE_SINCE_OFFSET_REQUIRED",
            };
            return Err(SetupReject { setup_id, reason });
        }
    }
    Ok(())
}

fn reject_checkpoint(
    setup_id: &'static str,
    favourite: OutcomeSide,
    checkpoint_favourite: Option<OutcomeSide>,
    params: &SetupGateParams,
) -> Result<(), SetupReject> {
    if !params.require_checkpoint_favourite {
        return Ok(());
    }
    match checkpoint_favourite {
        Some(cp) if cp == favourite => Ok(()),
        Some(_) => Err(SetupReject {
            setup_id,
            reason: "CHECKPOINT_FAVOURITE_MISMATCH",
        }),
        None => Err(SetupReject {
            setup_id,
            reason: "CHECKPOINT_MISSING",
        }),
    }
}

fn reject_momentum_and_signal(
    setup_id: &'static str,
    features: &SecondFeatures,
    params: &SetupGateParams,
) -> Result<(), SetupReject> {
    if params.require_positive_mom_med && features.mom_med_micros <= 0 {
        return Err(SetupReject {
            setup_id,
            reason: "MOMENTUM_NOT_POSITIVE",
        });
    }
    if params.require_mom_sign_agreement {
        if features.mom_med_micros < 0 || features.mom_long_micros < 0 {
            return Err(SetupReject {
                setup_id,
                reason: "MOMENTUM_DISAGREES",
            });
        }
        if features.mom_med_micros.signum() != features.mom_long_micros.signum()
            && features.mom_med_micros != 0
            && features.mom_long_micros != 0
        {
            return Err(SetupReject {
                setup_id,
                reason: "MOMENTUM_DISAGREES",
            });
        }
    }
    if params.min_confidence_delta_60s_micros > 0
        && features.confidence_delta_60s_micros < params.min_confidence_delta_60s_micros
    {
        return Err(SetupReject {
            setup_id,
            reason: "ACCELERATION_TOO_WEAK",
        });
    }
    if params.require_reference_agrees && !features.reference_agrees_with_favourite {
        return Err(SetupReject {
            setup_id,
            reason: "BTC_DISAGREES",
        });
    }
    if params.max_confidence_micros > 0
        && !(params.min_confidence_micros..=params.max_confidence_micros)
            .contains(&features.confidence_micros)
    {
        return Err(SetupReject {
            setup_id,
            reason: "CONFIDENCE_OUT_OF_BAND",
        });
    }
    Ok(())
}

fn maybe_demote_shadow(
    setup_id: &'static str,
    params: &SetupGateParams,
    candidate: &mut EntryCandidate,
) {
    let demote = params.force_shadow_book
        || (params.shadow_ask_threshold_micros > 0
            && candidate.pricing.ask_micros > params.shadow_ask_threshold_micros);
    if !demote {
        return;
    }
    candidate.experimental = true;
    candidate.book = PerformanceBook::ShadowExperiment;
    candidate.confidence_label = if setup_id.starts_with("1h_minute50") {
        ConfidenceLabel::Shadow
    } else {
        ConfidenceLabel::Experimental
    };
}

#[derive(Clone, Copy, Debug)]
struct SideCheckParams<'a> {
    setup_id: &'static str,
    snapshot: &'a MarketSecondSnapshot,
    features: &'a SecondFeatures,
    cfg: &'a WindowConfig,
    side: OutcomeSide,
    min_confidence: i64,
    max_ask_override: Option<i64>,
    require_non_negative_mom: bool,
}

fn base_side_checks(p: SideCheckParams<'_>) -> Result<EntryCandidate, SetupReject> {
    let setup_id = p.setup_id;
    let snapshot = p.snapshot;
    let features = p.features;
    let cfg = p.cfg;
    let side = p.side;
    let ask = side_ask(snapshot, side);
    let spread = side_spread(snapshot, side).ok_or(SetupReject {
        setup_id,
        reason: "MISSING_SPREAD",
    })?;
    if let Some(reason) = ask_policy_rejects(ask, false) {
        return Err(SetupReject { setup_id, reason });
    }
    let max_ask = p.max_ask_override.unwrap_or(cfg.max_ask_micros);
    if ask < cfg.preferred_ask_lo_micros || ask > max_ask {
        return Err(SetupReject {
            setup_id,
            reason: "ASK_OUT_OF_BAND",
        });
    }
    if !price_band_ok(cfg, ask) && p.max_ask_override.is_none() {
        return Err(SetupReject {
            setup_id,
            reason: "ASK_OUT_OF_BAND",
        });
    }
    if ask > max_ask {
        return Err(SetupReject {
            setup_id,
            reason: "ASK_ABOVE_CAP",
        });
    }
    if spread > cfg.max_spread_micros {
        return Err(SetupReject {
            setup_id,
            reason: "SPREAD_TOO_WIDE",
        });
    }
    let confidence = if side == features.favourite {
        features.confidence_micros
    } else {
        return Err(SetupReject {
            setup_id,
            reason: "NOT_FAVOURITE",
        });
    };
    if confidence < p.min_confidence {
        return Err(SetupReject {
            setup_id,
            reason: "CONFIDENCE_TOO_LOW",
        });
    }
    if !side_reference_agrees(side, features.reference_return_bps) {
        return Err(SetupReject {
            setup_id,
            reason: "REFERENCE_DISAGREES",
        });
    }
    if p.require_non_negative_mom && (features.mom_med_micros < 0 || features.mom_long_micros < 0) {
        return Err(SetupReject {
            setup_id,
            reason: "MOMENTUM_NEGATIVE",
        });
    }
    let calibrated = calibrated_probability(
        confidence,
        features.persistence_secs,
        features.mom_med_micros,
        features.mom_long_micros,
        features.reference_agrees_with_favourite,
        features.confidence_delta_60s_micros,
    );
    let pricing = compute_pricing(ask, calibrated, snapshot).ok_or(SetupReject {
        setup_id,
        reason: "PRICING_OVERFLOW",
    })?;
    if !edge_passes(cfg, pricing.edge_micros) {
        return Err(SetupReject {
            setup_id,
            reason: "EDGE_TOO_LOW",
        });
    }
    let experimental = matches!(
        cfg.label,
        ConfidenceLabel::Shadow | ConfidenceLabel::Experimental
    );
    let book = if experimental {
        PerformanceBook::ShadowExperiment
    } else {
        PerformanceBook::ActiveCore
    };
    Ok(EntryCandidate {
        setup_id: setup_id.to_owned(),
        side,
        pricing,
        confidence_label: cfg.label,
        book,
        experimental,
    })
}
