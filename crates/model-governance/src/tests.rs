use super::*;
use proptest::prelude::*;

fn fold(kind: FoldKind, start: i64, end: i64, byte: u8) -> WalkForwardFold {
    WalkForwardFold {
        kind,
        start_available_time_ns: start,
        end_available_time_ns: end,
        data_digest: [byte; 32],
        fold_digest: [0; 32],
    }
    .sealed()
}

fn plan() -> WalkForwardPlan {
    WalkForwardPlan {
        train: fold(FoldKind::Train, 0, 10, 1),
        validation: fold(FoldKind::Validation, 10, 20, 2),
        test: fold(FoldKind::Test, 20, 30, 3),
        plan_digest: [0; 32],
    }
    .sealed()
}

fn policy() -> GovernancePolicy {
    GovernancePolicy {
        minimum_observations: 100,
        minimum_net_pnl_micros: 1,
        minimum_challenger_improvement_micros: 2,
        maximum_drawdown_micros: 100,
        maximum_cvar_loss_micros: 100,
        minimum_fill_rate_bps: 1_000,
        minimum_data_coverage_bps: 9_000,
        maximum_hedge_failures: 1,
        maximum_model_drift_bps: 200,
        maximum_model_age_ns: 1_000,
        policy_digest: [0; 32],
    }
    .sealed()
}

fn evidence(model_byte: u8, pnl: i64) -> ModelEvidence {
    let plan = plan();
    ModelEvidence {
        artifact: ModelArtifact {
            model_id: ModelId([model_byte; 32]),
            label: format!("model-{model_byte}"),
            training_fold_digest: plan.train.fold_digest,
            validation_fold_digest: plan.validation.fold_digest,
            feature_schema_digest: [4; 32],
            configuration_digest: [5; 32],
            code_digest: [6; 32],
            trained_at_ns: 10,
            frozen_at_ns: 20,
            artifact_digest: [0; 32],
        }
        .sealed(),
        test_fold_digest: plan.test.fold_digest,
        evaluated_at_ns: 30,
        model_drift_bps: 10,
        metrics: EvaluationMetrics {
            net_pnl_micros: pnl,
            max_drawdown_micros: 10,
            cvar_loss_micros: 10,
            fees_micros: 1,
            slippage_micros: 1,
            fill_rate_bps: 2_000,
            data_coverage_bps: 9_500,
            hedge_failures: 0,
            observations: 200,
        },
        roles: RoleAttestations {
            research_label: "research-a".into(),
            evaluation_label: "evaluation-b".into(),
            adversarial_label: "adversary-c".into(),
            adversarial_passed: true,
        },
        evidence_digest: [0; 32],
    }
    .sealed()
}

#[test]
fn challenger_requires_unseen_evidence_and_improvement() {
    let decision = govern(
        &policy(),
        &plan(),
        100,
        Some(&evidence(1, 10)),
        &evidence(2, 12),
    )
    .unwrap();
    assert_eq!(decision.outcome, GovernanceOutcome::PaperChampionCandidate);
    assert_eq!(decision.selected_model_id, Some(ModelId([2; 32])));
    assert!(decision.verify_digest());
    assert!(!decision.capital_authority && !decision.submission_authority);
}

#[test]
fn stale_or_leaking_model_is_rejected() {
    let mut leaking = evidence(1, 10);
    leaking.artifact.frozen_at_ns = 21;
    leaking.artifact = leaking.artifact.sealed();
    leaking = leaking.sealed();
    assert_eq!(
        govern(&policy(), &plan(), 100, None, &leaking),
        Err(GovernanceError::Artifact)
    );
}

#[test]
fn adversarial_failure_is_no_trade_not_promotion() {
    let mut candidate = evidence(1, 10);
    candidate.roles.adversarial_passed = false;
    candidate = candidate.sealed();
    let decision = govern(&policy(), &plan(), 100, None, &candidate).unwrap();
    assert_eq!(decision.outcome, GovernanceOutcome::NoTrade);
}

#[test]
fn duplicate_roles_are_rejected() {
    let mut candidate = evidence(1, 10);
    candidate.roles.evaluation_label = "research-a".into();
    candidate = candidate.sealed();
    assert_eq!(
        govern(&policy(), &plan(), 100, None, &candidate),
        Err(GovernanceError::Roles)
    );
}

#[test]
fn drift_forces_no_trade() {
    let mut candidate = evidence(1, 10);
    candidate.model_drift_bps = 201;
    candidate = candidate.sealed();
    let decision = govern(&policy(), &plan(), 100, None, &candidate).unwrap();
    assert_eq!(decision.outcome, GovernanceOutcome::NoTrade);
}

#[test]
fn tampered_evidence_is_rejected() {
    let mut candidate = evidence(1, 10);
    candidate.metrics.net_pnl_micros = 99;
    assert_eq!(
        govern(&policy(), &plan(), 100, None, &candidate),
        Err(GovernanceError::Evidence)
    );
}

#[test]
fn overlapping_folds_and_duplicate_model_identity_are_rejected() {
    let mut invalid_plan = plan();
    invalid_plan.validation.start_available_time_ns = 9;
    invalid_plan = invalid_plan.sealed();
    assert_eq!(
        govern(&policy(), &invalid_plan, 100, None, &evidence(1, 10)),
        Err(GovernanceError::Folds)
    );
    let duplicate = evidence(1, 20);
    assert_eq!(
        govern(&policy(), &plan(), 100, Some(&evidence(1, 10)), &duplicate),
        Err(GovernanceError::Evidence)
    );
}

#[test]
fn missing_provenance_and_negative_clock_are_rejected() {
    let mut candidate = evidence(1, 10);
    candidate.artifact.code_digest = [0; 32];
    candidate.artifact = candidate.artifact.sealed();
    candidate = candidate.sealed();
    assert_eq!(
        govern(&policy(), &plan(), 100, None, &candidate),
        Err(GovernanceError::Artifact)
    );
    assert_eq!(
        govern(&policy(), &plan(), -1, None, &evidence(2, 10)),
        Err(GovernanceError::Time)
    );
}

proptest! {
    #[test]
    fn out_of_policy_drift_never_promotes(drift in 201_u16..=u16::MAX) {
        let mut candidate = evidence(1, 10);
        candidate.model_drift_bps = drift;
        candidate = candidate.sealed();
        let result = govern(&policy(), &plan(), 100, None, &candidate);
        match result {
            Ok(decision) => prop_assert_eq!(decision.outcome, GovernanceOutcome::NoTrade),
            Err(error) => prop_assert_eq!(error, GovernanceError::Metrics),
        }
    }
}
