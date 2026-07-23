use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use shadow_session_campaign::{EvidenceReason, RequiredScenario};
use tempfile::tempdir;

const CREATED: i64 = 1_000_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> GovernancePolicy {
    GovernancePolicy {
        max_bundles: 8,
        minimum_campaigns: 3,
        minimum_distinct_manifests: 2,
        minimum_distinct_schedules: 2,
        minimum_distinct_final_states: 2,
        minimum_total_sessions: 6,
        minimum_total_steps: 30,
        minimum_total_fault_cycles: 9,
        minimum_regression_retention_bps: 9_000,
        maximum_bundle_age_ns: 1_000,
        maximum_candidate_age_ns: 1_000,
        maximum_decision_age_ns: 500,
    }
}

fn seal_bundle(mut bundle: OperatorEvidenceBundle) -> OperatorEvidenceBundle {
    bundle.bundle_digest = [0; 32];
    let serialized = serde_json::to_vec(&bundle).expect("bundle JSON");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"shadow-session-evidence-v1");
    hasher.update(&(serialized.len() as u64).to_le_bytes());
    hasher.update(&serialized);
    bundle.bundle_digest = *hasher.finalize().as_bytes();
    bundle
}

fn eligible_bundle(profile: u8, evaluated_at_ns: i64) -> OperatorEvidenceBundle {
    seal_bundle(OperatorEvidenceBundle {
        bundle_id: bytes(profile),
        campaign_id: bytes(profile.saturating_add(10)),
        manifest_digest: bytes(profile.saturating_add(20)),
        schedule_digest: bytes(profile.saturating_add(30)),
        evaluated_at_ns,
        status: CampaignStatus::PromotionEligible,
        reasons: Vec::new(),
        required_scenarios: vec![RequiredScenario::DeadMan],
        covered_scenarios: vec![RequiredScenario::DeadMan],
        session_count: 2,
        completed_session_count: 2,
        applied_step_count: 10,
        certification_install_count: 2,
        dead_man_count: 1,
        restart_count: 1,
        unknown_recovery_count: 1,
        initial_gateway_digest: bytes(profile.saturating_add(40)),
        final_gateway_digest: bytes(profile.saturating_add(50)),
        final_cash_reserved_micros: 0,
        final_pending_conversion_count: 0,
        final_gateway_ready: true,
        operator_decision_required: true,
        promotion_authority_granted: false,
        deployment_authority_granted: false,
        bundle_digest: [0; 32],
    })
}

fn baseline() -> RegressionBaseline {
    RegressionBaseline {
        baseline_id: bytes(80),
        campaign_count: 3,
        total_sessions: 6,
        total_steps: 30,
        total_fault_cycles: 9,
        baseline_digest: [0; 32],
    }
    .sealed()
}

fn artifacts() -> ReleaseArtifacts {
    ReleaseArtifacts {
        release_id: bytes(81),
        source_digest: bytes(82),
        binary_digest: bytes(83),
        toolchain_digest: bytes(84),
        dependency_lock_digest: bytes(85),
        sbom_digest: bytes(86),
        configuration_digest: bytes(87),
        artifacts_digest: [0; 32],
    }
    .sealed()
}

fn rollback() -> RollbackCriteria {
    RollbackCriteria {
        criteria_id: bytes(88),
        rollback_target_digest: bytes(89),
        maximum_canary_duration_ns: 100,
        maximum_unreconciled_ns: 10,
        maximum_unknown_state_ns: 10,
        maximum_session_loss_micros: 100_000,
        maximum_consecutive_faults: 2,
        require_capital_floor_halt: true,
        require_reconciliation_halt: true,
        criteria_digest: [0; 32],
    }
    .sealed()
}

fn submission() -> ReleaseCandidateSubmission {
    ReleaseCandidateSubmission {
        candidate_id: bytes(90),
        created_at_ns: CREATED,
        expires_at_ns: CREATED + 900,
        evidence: vec![
            eligible_bundle(1, CREATED - 30),
            eligible_bundle(2, CREATED - 20),
            eligible_bundle(3, CREATED - 10),
        ],
        baseline: baseline(),
        artifacts: artifacts(),
        rollback: rollback(),
        policy_digest: [0; 32],
        evidence_set_digest: [0; 32],
        candidate_digest: [0; 32],
    }
    .sealed(&policy())
}

fn decision(
    submission: &ReleaseCandidateSubmission,
    role: DecisionRole,
    operator: u8,
    verdict: DecisionVerdict,
    at: i64,
) -> OperatorDecision {
    let role_offset = match role {
        DecisionRole::Risk => 100,
        DecisionRole::Release => 150,
    };
    OperatorDecision {
        decision_id: bytes(operator.saturating_add(role_offset)),
        operator_id: bytes(operator),
        role,
        verdict,
        candidate_digest: submission.candidate_digest,
        evidence_set_digest: submission.evidence_set_digest,
        artifacts_digest: submission.artifacts.artifacts_digest,
        rollback_digest: submission.rollback.criteria_digest,
        decided_at_ns: at,
        valid_until_ns: at + 400,
        decision_digest: [0; 32],
    }
    .sealed()
}

fn commands() -> Vec<GovernanceCommand> {
    let submission = submission();
    vec![
        GovernanceCommand::RegisterCandidate {
            command_id: GovernanceCommandId(bytes(1)),
            submission: Box::new(submission.clone()),
            recorded_at_ns: CREATED,
        },
        GovernanceCommand::RecordDecision {
            command_id: GovernanceCommandId(bytes(2)),
            candidate_id: submission.candidate_id,
            decision: Box::new(decision(
                &submission,
                DecisionRole::Risk,
                20,
                DecisionVerdict::Approve,
                CREATED + 10,
            )),
            recorded_at_ns: CREATED + 10,
        },
        GovernanceCommand::RecordDecision {
            command_id: GovernanceCommandId(bytes(3)),
            candidate_id: submission.candidate_id,
            decision: Box::new(decision(
                &submission,
                DecisionRole::Release,
                21,
                DecisionVerdict::Approve,
                CREATED + 20,
            )),
            recorded_at_ns: CREATED + 20,
        },
        GovernanceCommand::Finalize {
            command_id: GovernanceCommandId(bytes(4)),
            candidate_id: submission.candidate_id,
            record_id: bytes(91),
            evaluated_at_ns: CREATED + 30,
            recorded_at_ns: CREATED + 30,
        },
    ]
}

fn run_commands(commands: &[GovernanceCommand]) -> PromotionGovernance {
    let mut governance = PromotionGovernance::new(policy()).expect("governance");
    for command in commands {
        governance.apply(command).expect("command");
    }
    governance
}

#[test]
fn independent_campaigns_and_dual_control_produce_non_authorizing_eligibility() {
    let governance = run_commands(&commands());
    let record = governance.snapshot().last_record.expect("record");
    assert_eq!(record.status, CanaryStatus::CanaryEligible);
    assert!(record.reasons.is_empty());
    assert!(record.dual_control_complete);
    assert!(record.operator_execution_required);
    assert!(record.rollback_required_on_threshold);
    assert!(!record.canary_execution_authority_granted);
    assert!(!record.promotion_authority_granted);
    assert!(!record.deployment_authority_granted);
    assert!(!record.credential_authority_granted);
    assert!(!record.live_trading_authority_granted);
    assert!(record.verify_digest());
}

#[test]
fn duplicate_stale_and_ineligible_campaigns_cannot_inflate_aggregate() {
    let mut submission = submission();
    let duplicate = submission.evidence[0].clone();
    let mut stale = eligible_bundle(4, CREATED - 2_000);
    stale.evaluated_at_ns = CREATED - 2_000;
    stale = seal_bundle(stale);
    let mut ineligible = eligible_bundle(5, CREATED - 5);
    ineligible.status = CampaignStatus::NotEligible;
    ineligible.reasons = vec![EvidenceReason::GatewayNotReady];
    ineligible.final_gateway_ready = false;
    ineligible = seal_bundle(ineligible);
    submission.evidence = vec![duplicate.clone(), duplicate, stale, ineligible];
    submission = submission.sealed(&policy());
    let mut governance = PromotionGovernance::new(policy()).expect("governance");
    let outcome = governance
        .apply(&GovernanceCommand::RegisterCandidate {
            command_id: GovernanceCommandId(bytes(6)),
            submission: Box::new(submission),
            recorded_at_ns: CREATED,
        })
        .expect("register");
    let GovernanceDetail::CandidateRegistered {
        aggregate, reasons, ..
    } = outcome.detail
    else {
        panic!("registered detail")
    };
    assert_eq!(aggregate.unique_campaigns, 1);
    assert!(reasons.contains(&EligibilityReason::DuplicateEvidence));
    assert!(reasons.iter().any(|reason| matches!(
        reason,
        EligibilityReason::CampaignStale(_) | EligibilityReason::CampaignNotEligible(_)
    )));
    assert!(reasons.contains(&EligibilityReason::InsufficientCampaigns));
}

#[test]
fn regression_thresholds_are_attributable() {
    let mut submission = submission();
    submission.baseline = RegressionBaseline {
        baseline_id: bytes(92),
        campaign_count: 5,
        total_sessions: 20,
        total_steps: 100,
        total_fault_cycles: 40,
        baseline_digest: [0; 32],
    }
    .sealed();
    submission = submission.sealed(&policy());
    let mut governance = PromotionGovernance::new(policy()).expect("governance");
    let outcome = governance
        .apply(&GovernanceCommand::RegisterCandidate {
            command_id: GovernanceCommandId(bytes(7)),
            submission: Box::new(submission),
            recorded_at_ns: CREATED,
        })
        .expect("register");
    let GovernanceDetail::CandidateRegistered { reasons, .. } = outcome.detail else {
        panic!("registered detail")
    };
    assert!(reasons.contains(&EligibilityReason::RegressionCampaignCount));
    assert!(reasons.contains(&EligibilityReason::RegressionSessionCount));
    assert!(reasons.contains(&EligibilityReason::RegressionStepCount));
    assert!(reasons.contains(&EligibilityReason::RegressionFaultCycles));
}

#[test]
fn same_operator_rejection_and_expiry_deny_dual_control() {
    let submission = submission();
    let cases = [
        (
            decision(
                &submission,
                DecisionRole::Risk,
                30,
                DecisionVerdict::Approve,
                CREATED + 10,
            ),
            decision(
                &submission,
                DecisionRole::Release,
                30,
                DecisionVerdict::Approve,
                CREATED + 20,
            ),
            CREATED + 30,
            EligibilityReason::SameOperator,
        ),
        (
            decision(
                &submission,
                DecisionRole::Risk,
                31,
                DecisionVerdict::Reject,
                CREATED + 10,
            ),
            decision(
                &submission,
                DecisionRole::Release,
                32,
                DecisionVerdict::Approve,
                CREATED + 20,
            ),
            CREATED + 30,
            EligibilityReason::RejectedDecision(DecisionRole::Risk),
        ),
        (
            decision(
                &submission,
                DecisionRole::Risk,
                33,
                DecisionVerdict::Approve,
                CREATED + 10,
            ),
            decision(
                &submission,
                DecisionRole::Release,
                34,
                DecisionVerdict::Approve,
                CREATED + 20,
            ),
            CREATED + 500,
            EligibilityReason::ExpiredDecision(DecisionRole::Risk),
        ),
    ];
    for (index, (risk, release, evaluated_at, expected)) in cases.into_iter().enumerate() {
        let mut governance = PromotionGovernance::new(policy()).expect("governance");
        governance
            .apply(&GovernanceCommand::RegisterCandidate {
                command_id: GovernanceCommandId(bytes(40)),
                submission: Box::new(submission.clone()),
                recorded_at_ns: CREATED,
            })
            .expect("register");
        for (id, item) in [(41, risk), (42, release)] {
            governance
                .apply(&GovernanceCommand::RecordDecision {
                    command_id: GovernanceCommandId(bytes(id)),
                    candidate_id: submission.candidate_id,
                    recorded_at_ns: item.decided_at_ns,
                    decision: Box::new(item),
                })
                .expect("decision");
        }
        governance
            .apply(&GovernanceCommand::Finalize {
                command_id: GovernanceCommandId(bytes(43)),
                candidate_id: submission.candidate_id,
                record_id: bytes(u8::try_from(index + 120).expect("record id")),
                evaluated_at_ns: evaluated_at,
                recorded_at_ns: evaluated_at,
            })
            .expect("finalize");
        let record = governance.snapshot().last_record.expect("record");
        assert_eq!(record.status, CanaryStatus::NotEligible);
        assert!(record.reasons.contains(&expected));
        assert!(!record.dual_control_complete);
    }
}

#[test]
fn artifact_substitution_halts_before_candidate_installation() {
    let mut submission = submission();
    submission.artifacts.configuration_digest = bytes(200);
    let mut governance = PromotionGovernance::new(policy()).expect("governance");
    assert!(matches!(
        governance.apply(&GovernanceCommand::RegisterCandidate {
            command_id: GovernanceCommandId(bytes(50)),
            submission: Box::new(submission),
            recorded_at_ns: CREATED,
        }),
        Err(Error::Candidate)
    ));
    let snapshot = governance.snapshot();
    assert!(snapshot.halted);
    assert!(snapshot.candidate_id.is_none());
}

#[test]
fn canary_record_file_is_create_new_checksummed_and_digest_verified() {
    let governance = run_commands(&commands());
    let record = governance.snapshot().last_record.expect("record");
    let directory = tempdir().expect("dir");
    let path = directory.path().join("canary.record");
    write_canary_record_create_new(&path, &record).expect("write");
    assert_eq!(read_canary_record(&path).expect("read"), record);
    assert!(write_canary_record_create_new(&path, &record).is_err());
    let mut bytes = std::fs::read(&path).expect("bytes");
    let index = bytes.len() - 1;
    bytes[index] ^= 1;
    std::fs::write(&path, bytes).expect("corrupt");
    assert!(matches!(
        read_canary_record(&path),
        Err(CanaryRecordFileError::Checksum)
    ));
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
}

impl EventJournal for FailingJournal {
    fn append_event(
        &mut self,
        envelope: &event_schema::EventEnvelope,
    ) -> Result<u64, JournalBackendError> {
        self.last = Some(envelope.sequence);
        Ok(0)
    }

    fn sync_events(&self) -> Result<(), JournalBackendError> {
        Err(JournalBackendError::Single(
            market_recorder::JournalError::Io(std::io::Error::other("sync failure")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn durable_replay_checkpoint_and_sync_failure_are_fail_closed() {
    let commands = commands();
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 512 * 1024,
            max_segment_records: 2,
        },
    )
    .expect("writer");
    let recovery = GovernanceRecovery {
        governance: PromotionGovernance::new(policy()).expect("governance"),
        last_sequence: None,
    };
    let mut durable = DurableGovernance::new(writer, recovery).expect("durable");
    for command in &commands {
        durable.apply(command).expect("command");
    }
    let expected = durable.governance().snapshot().digest;
    let checkpoint = GovernanceCheckpoint {
        sequence: u64::try_from(commands.len() - 1).expect("sequence"),
        governance_digest: expected,
    };
    let checkpoint_path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).expect("checkpoint");
    assert_eq!(read_checkpoint(&checkpoint_path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.governance.snapshot().digest, expected);

    let recovery = GovernanceRecovery {
        governance: PromotionGovernance::new(policy()).expect("governance"),
        last_sequence: None,
    };
    let mut failing = DurableGovernance::new(FailingJournal::default(), recovery).expect("new");
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(GovernanceStorageError::Journal(_))
    ));
    assert!(matches!(
        failing.apply(&commands[0]),
        Err(GovernanceStorageError::Halted(_))
    ));
    assert_eq!(failing.governance().snapshot().accepted_commands, 0);
}

#[test]
fn identical_governance_commands_have_identical_complete_digests() {
    let commands = commands();
    let first = run_commands(&commands);
    let second = run_commands(&commands);
    assert_eq!(first.snapshot().digest, second.snapshot().digest);
    assert_eq!(first.snapshot().last_record, second.snapshot().last_record);
}

proptest! {
    #[test]
    fn stricter_retention_never_lowers_the_required_floor(
        baseline in 1_u64..1_000_000,
        lower in 1_u64..=9_999,
        increase in 0_u64..=1,
    ) {
        let higher = lower.saturating_add(increase).min(BASIS_POINTS);
        let low = retained_floor(baseline, lower).expect("low");
        let high = retained_floor(baseline, higher).expect("high");
        prop_assert!(high >= low);
    }
}
