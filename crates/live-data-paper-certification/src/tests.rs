use super::*;
use continuous_shadow_certification::{CampaignReport, CampaignReportStatus, CampaignScenario};
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;
fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> PaperPolicy {
    PaperPolicy {
        maximum_campaign_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_record_age_ns: 100,
        maximum_records: 10,
        maximum_latency_ns: 1_000,
        minimum_fold_evaluations: 1,
    }
}
fn upstream() -> CampaignReport {
    CampaignReport {
        report_id: id(1),
        plan_digest: id(2),
        chain_report_digest: id(3),
        final_tick_digest: id(4),
        accelerated_duration_ns: 10_800_000_000_000,
        real_elapsed_duration_ns: 1_000,
        rollover_count: 2,
        covered_scenarios: CampaignScenario::ALL.to_vec(),
        operations_operator_digest: id(5),
        risk_operator_digest: id(6),
        finalized_at_ns: 100,
        status: CampaignReportStatus::LocallyCertified,
        real_multi_day_environment_certified: false,
        credential_material_created: false,
        external_connection_opened: false,
        external_mutation_observed: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn fold(kind: FoldKind, start: i64, end: i64) -> WalkForwardFold {
    WalkForwardFold {
        kind,
        start_available_time_ns: start,
        end_available_time_ns: end,
        fold_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> PaperPlan {
    PaperPlan {
        plan_id: id(10),
        campaign_report: upstream(),
        capture_manifest_digest: id(11),
        strategy_digest: id(12),
        expected_record_count: 3,
        folds: vec![
            fold(FoldKind::Train, 205, 214),
            fold(FoldKind::Validation, 215, 224),
            fold(FoldKind::Test, 225, 240),
        ],
        required_scenarios: PaperScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn record(b: u8, sequence: u64, at: i64) -> CapturedRecord {
    CapturedRecord {
        record_id: id(b),
        sequence,
        event_time_ns: at - 2,
        received_time_ns: at - 1,
        available_time_ns: at,
        provenance_digest: id(b + 1),
        payload_digest: id(b + 2),
        record_digest: [0; 32],
    }
    .sealed()
}
fn evaluation(b: u8, fold: FoldKind, decision: i64, sequence: u64) -> PaperEvaluation {
    PaperEvaluation {
        evaluation_id: id(b),
        fold,
        decision_time_ns: decision,
        consumed_sequences: vec![sequence],
        queue_cases: QueueCase::ALL.to_vec(),
        outcomes: PaperOutcome::ALL.to_vec(),
        latency: LatencyProfile {
            signal_ns: 10,
            submission_ns: 20,
            acknowledgement_ns: 30,
            cancellation_ns: 40,
        },
        price_touch_only_fill: false,
        unknown_retains_reservation: true,
        proposal_digest: id(b + 1),
        risk_digest: id(b + 2),
        reservation_digest: id(b + 3),
        execution_digest: id(b + 4),
        settlement_digest: id(b + 5),
        accounting_digest: id(b + 6),
        external_mutation_observed: false,
        evaluation_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> LiveDataPaperCertification {
    let mut owner = LiveDataPaperCertification::new(policy()).unwrap();
    owner
        .apply(&PaperCommand::Register {
            command_id: PaperCommandId(id(20)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}
fn ingested() -> LiveDataPaperCertification {
    let mut owner = registered();
    for (b, seq, at) in [(30, 1, 210), (33, 2, 220), (36, 3, 230)] {
        owner
            .apply(&PaperCommand::Ingest {
                command_id: PaperCommandId(id(b)),
                record: record(b + 1, seq, at),
                recorded_at_ns: at,
            })
            .unwrap();
    }
    owner
        .apply(&PaperCommand::FreezeStrategy {
            command_id: PaperCommandId(id(40)),
            strategy_digest: plan().strategy_digest,
            frozen_at_ns: 224,
            recorded_at_ns: 231,
        })
        .unwrap();
    owner
}

#[test]
fn complete_walk_forward_campaign_is_local_and_non_authorizing() {
    let mut owner = ingested();
    for (b, fold, decision, sequence) in [
        (50, FoldKind::Train, 212, 1),
        (60, FoldKind::Validation, 222, 2),
        (70, FoldKind::Test, 235, 3),
    ] {
        owner
            .apply(&PaperCommand::Evaluate {
                command_id: PaperCommandId(id(b)),
                evaluation: Box::new(evaluation(b + 1, fold, decision, sequence)),
                recorded_at_ns: 232 + i64::from((b - 50) / 10),
            })
            .unwrap();
    }
    let result = owner
        .apply(&PaperCommand::Finalize {
            command_id: PaperCommandId(id(80)),
            report_id: id(81),
            finalized_at_ns: 240,
            recorded_at_ns: 240,
        })
        .unwrap();
    let report = match result.detail {
        PaperDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert_eq!(report.covered_scenarios, PaperScenario::ALL);
    assert_eq!(report.evaluation_count, 3);
    assert!(
        !report.real_pnl_observed
            && !report.credential_material_created
            && !report.external_connection_opened
            && !report.external_mutation_observed
            && !report.capital_authority_granted
            && !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn future_available_record_and_price_touch_fill_halt() {
    let mut owner = ingested();
    let mut future = evaluation(90, FoldKind::Train, 209, 1);
    future = future.sealed();
    assert_eq!(
        owner
            .apply(&PaperCommand::Evaluate {
                command_id: PaperCommandId(id(91)),
                evaluation: Box::new(future),
                recorded_at_ns: 232
            })
            .unwrap_err(),
        Error::Evaluation
    );
    assert!(owner.is_halted());
    let mut owner = ingested();
    let mut touched = evaluation(92, FoldKind::Train, 212, 1);
    touched.price_touch_only_fill = true;
    touched = touched.sealed();
    assert_eq!(
        owner
            .apply(&PaperCommand::Evaluate {
                command_id: PaperCommandId(id(93)),
                evaluation: Box::new(touched),
                recorded_at_ns: 232
            })
            .unwrap_err(),
        Error::Evaluation
    );
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
}
impl EventJournal for FailingJournal {
    fn append_event(
        &mut self,
        e: &event_schema::EventEnvelope,
    ) -> Result<u64, JournalBackendError> {
        self.last = Some(e.sequence);
        Ok(0)
    }
    fn sync_events(&self) -> Result<(), JournalBackendError> {
        Err(JournalBackendError::Single(JournalError::Io(
            std::io::Error::other("sync"),
        )))
    }
    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}
#[test]
fn journal_checkpoint_report_and_sync_failure_are_fail_closed() {
    let dir = tempdir().unwrap();
    let segments = dir.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .unwrap();
    let recovery = PaperRecovery {
        owner: LiveDataPaperCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurablePaperCertification::new(writer, recovery).unwrap();
    let command = PaperCommand::Register {
        command_id: PaperCommandId(id(100)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = PaperCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let path = dir.path().join("checkpoint");
    write_checkpoint_create_new(&path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
    assert_eq!(
        recover_segmented(&segments, policy(), Some(checkpoint))
            .unwrap()
            .owner
            .snapshot()
            .digest,
        expected
    );
    let report = PaperReport {
        report_id: id(101),
        plan_digest: id(102),
        campaign_report_digest: id(103),
        capture_manifest_digest: id(104),
        strategy_digest: id(105),
        record_count: 3,
        evaluation_count: 3,
        covered_scenarios: PaperScenario::ALL.to_vec(),
        finalized_at_ns: 300,
        status: PaperReportStatus::LocallyCertified,
        real_pnl_observed: false,
        credential_material_created: false,
        external_connection_opened: false,
        external_mutation_observed: false,
        capital_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed();
    let path = dir.path().join("report");
    write_report_create_new(&path, &report).unwrap();
    assert_eq!(read_report(&path).unwrap(), report);
    assert!(write_report_create_new(&path, &report).is_err());
    let recovery = PaperRecovery {
        owner: LiveDataPaperCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurablePaperCertification::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(PaperStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(PaperStorageError::Halted(_))
    ));
}

proptest! { #[test] fn over_limit_latency_never_accepts(extra in 1_i64..100_000) { let mut owner = ingested(); let mut value = evaluation(120, FoldKind::Train, 212, 1); value.latency.submission_ns = policy().maximum_latency_ns + extra; value = value.sealed(); prop_assert_eq!(owner.apply(&PaperCommand::Evaluate { command_id: PaperCommandId(id(121)), evaluation: Box::new(value), recorded_at_ns: 232 }).unwrap_err(), Error::Evaluation); } }
