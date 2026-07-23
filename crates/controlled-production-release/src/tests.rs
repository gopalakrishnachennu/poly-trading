use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use micro_capital_canary_controller::{CanaryReport, CanaryReportStatus, CanaryScenario};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(value: u8) -> [u8; 32] {
    [value; 32]
}

fn policy() -> ReleasePolicy {
    ReleasePolicy {
        maximum_canary_report_age_ns: 1_000,
        maximum_plan_lifetime_ns: 1_000,
        maximum_evidence_age_ns: 100,
        maximum_cases: 20,
        minimum_regions: 2,
    }
}

fn upstream() -> CanaryReport {
    CanaryReport {
        report_id: id(1),
        plan_digest: id(2),
        auth_report_digest: id(3),
        covered_scenarios: CanaryScenario::ALL.to_vec(),
        finalized_at_ns: 100,
        status: CanaryReportStatus::CodeEligible,
        live_canary_complete: false,
        legal_eligibility_confirmed: false,
        real_capital_allocated: false,
        credential_material_created: false,
        signature_produced: false,
        external_order_submitted: false,
        capital_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}

fn subjects() -> ReleaseSubjects {
    ReleaseSubjects {
        release_digest: id(10),
        artifact_digest: id(11),
        configuration_digest: id(12),
        infrastructure_digest: id(13),
        reconciliation_digest: id(14),
        incident_runbook_digest: id(15),
        disaster_recovery_digest: id(16),
        rollback_digest: id(17),
    }
}

fn plan() -> ReleasePlan {
    ReleasePlan {
        plan_id: id(18),
        canary_report: upstream(),
        subjects: subjects(),
        capital_stages: vec![
            CapitalStage {
                index: 0,
                capital_ceiling_micros: 100,
                exposure_ceiling_micros: 50,
                session_loss_ceiling_micros: 5,
            },
            CapitalStage {
                index: 1,
                capital_ceiling_micros: 200,
                exposure_ceiling_micros: 100,
                session_loss_ceiling_micros: 10,
            },
            CapitalStage {
                index: 2,
                capital_ceiling_micros: 400,
                exposure_ceiling_micros: 200,
                session_loss_ceiling_micros: 20,
            },
        ],
        required_regions: vec![id(20), id(21)],
        required_scenarios: ReleaseScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 900,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn region(value: u8) -> RegionHealth {
    RegionHealth {
        evidence_id: id(value + 20),
        region_digest: id(value),
        sequence: 1,
        observed_at_ns: 450,
        healthy: true,
        reconciliation_current: true,
        capital_floor_intact: true,
        no_unknown_state: true,
        external_mutation_observed: false,
        evidence_digest: [0; 32],
    }
    .sealed()
}

fn case(value: u8, sequence: u64, scenario: ReleaseScenario) -> ReleaseCase {
    let mut case = ReleaseCase {
        case_id: id(value),
        sequence,
        scenario,
        stage_index: 0,
        observed_at_ns: 400,
        disposition: ReleaseDisposition::CodeEligible,
        reconciliation_current: false,
        incident_process_proven: false,
        disaster_recovery_proven: false,
        rollback_proven: false,
        revocation_proven: false,
        no_trade_available: true,
        external_action_observed: false,
        case_digest: [0; 32],
    };
    match scenario {
        ReleaseScenario::NoTrade => case.disposition = ReleaseDisposition::NoTrade,
        ReleaseScenario::ContinuousReconciliation => case.reconciliation_current = true,
        ReleaseScenario::EvidenceExpiry => {
            case.observed_at_ns = 0;
            case.disposition = ReleaseDisposition::NoTrade;
        }
        ReleaseScenario::IncidentResponse => {
            case.incident_process_proven = true;
            case.disposition = ReleaseDisposition::RollbackRequired;
        }
        ReleaseScenario::DisasterRecovery => {
            case.disaster_recovery_proven = true;
            case.disposition = ReleaseDisposition::RollbackRequired;
        }
        ReleaseScenario::Rollback => {
            case.rollback_proven = true;
            case.disposition = ReleaseDisposition::RollbackRequired;
        }
        ReleaseScenario::Revocation => {
            case.revocation_proven = true;
            case.disposition = ReleaseDisposition::NoTrade;
        }
        ReleaseScenario::StagedCeilings
        | ReleaseScenario::MultiRegionHealth
        | ReleaseScenario::Governance => {}
    }
    case.sealed()
}

fn approved() -> ControlledProductionRelease {
    let mut owner = ControlledProductionRelease::new(policy()).unwrap();
    owner
        .apply(&ReleaseCommand::Register {
            command_id: ReleaseCommandId(id(30)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
        .apply(&ReleaseCommand::Approve {
            command_id: ReleaseCommandId(id(31)),
            release_operator_digest: id(32),
            risk_operator_digest: id(33),
            operations_operator_digest: id(34),
            recorded_at_ns: 201,
        })
        .unwrap();
    owner
}

fn completed() -> ControlledReleaseReport {
    let mut owner = approved();
    owner
        .apply(&ReleaseCommand::RecordRegion {
            command_id: ReleaseCommandId(id(35)),
            evidence: region(20),
            recorded_at_ns: 450,
        })
        .unwrap();
    owner
        .apply(&ReleaseCommand::RecordRegion {
            command_id: ReleaseCommandId(id(36)),
            evidence: region(21),
            recorded_at_ns: 450,
        })
        .unwrap();
    for (index, scenario) in ReleaseScenario::ALL.into_iter().enumerate() {
        owner
            .apply(&ReleaseCommand::RecordCase {
                command_id: ReleaseCommandId(id(40 + u8::try_from(index).unwrap())),
                case: case(
                    60 + u8::try_from(index).unwrap(),
                    u64::try_from(index + 1).unwrap(),
                    scenario,
                ),
                recorded_at_ns: 460 + i64::try_from(index).unwrap(),
            })
            .unwrap();
    }
    match owner
        .apply(&ReleaseCommand::Finalize {
            command_id: ReleaseCommandId(id(80)),
            report_id: id(81),
            finalized_at_ns: 500,
            recorded_at_ns: 500,
        })
        .unwrap()
        .detail
    {
        ReleaseDetail::Finalized(report) => *report,
        _ => panic!("expected report"),
    }
}

#[test]
fn complete_release_control_matrix_is_code_only() {
    let report = completed();
    assert_eq!(report.covered_scenarios, ReleaseScenario::ALL);
    assert_eq!(report.covered_regions, vec![id(20), id(21)]);
    assert!(report.verify_digest());
    assert!(
        !report.target_environment_certified
            && !report.production_release_complete
            && !report.legal_eligibility_confirmed
            && !report.real_capital_allocated
            && !report.credential_material_created
            && !report.signature_produced
            && !report.external_order_submitted
            && !report.capital_authority_granted
            && !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn stale_region_distinct_control_and_revocation_fail_closed() {
    let mut owner = ControlledProductionRelease::new(policy()).unwrap();
    owner
        .apply(&ReleaseCommand::Register {
            command_id: ReleaseCommandId(id(90)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    assert_eq!(
        owner
            .apply(&ReleaseCommand::Approve {
                command_id: ReleaseCommandId(id(91)),
                release_operator_digest: id(92),
                risk_operator_digest: id(92),
                operations_operator_digest: id(93),
                recorded_at_ns: 201,
            })
            .unwrap_err(),
        Error::Approval
    );

    let mut owner = approved();
    let mut stale = region(20);
    stale.observed_at_ns = 100;
    stale = stale.sealed();
    assert_eq!(
        owner
            .apply(&ReleaseCommand::RecordRegion {
                command_id: ReleaseCommandId(id(94)),
                evidence: stale,
                recorded_at_ns: 450
            })
            .unwrap_err(),
        Error::Region
    );

    let mut owner = approved();
    owner
        .apply(&ReleaseCommand::Revoke {
            command_id: ReleaseCommandId(id(95)),
            reason_digest: id(96),
            recorded_at_ns: 300,
        })
        .unwrap();
    assert!(owner.snapshot().revoked);
    assert_eq!(
        owner
            .apply(&ReleaseCommand::Finalize {
                command_id: ReleaseCommandId(id(97)),
                report_id: id(98),
                finalized_at_ns: 301,
                recorded_at_ns: 301
            })
            .unwrap_err(),
        Error::Finalize
    );
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
}

impl EventJournal for FailingJournal {
    fn append_event(
        &mut self,
        event: &event_schema::EventEnvelope,
    ) -> Result<u64, JournalBackendError> {
        self.last = Some(event.sequence);
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
fn journal_checkpoint_report_and_sync_are_fail_closed() {
    let directory = tempdir().unwrap();
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .unwrap();
    let recovery = ReleaseRecovery {
        owner: ControlledProductionRelease::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableReleaseController::new(writer, recovery).unwrap();
    let command = ReleaseCommand::Register {
        command_id: ReleaseCommandId(id(100)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = ReleaseCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = directory.path().join("checkpoint");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    assert_eq!(
        recover_segmented(&segments, policy(), Some(checkpoint))
            .unwrap()
            .owner
            .snapshot()
            .digest,
        expected
    );

    let report = completed();
    let report_path = directory.path().join("report");
    write_report_create_new(&report_path, &report).unwrap();
    assert_eq!(read_report(&report_path).unwrap(), report);

    let recovery = ReleaseRecovery {
        owner: ControlledProductionRelease::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableReleaseController::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(ReleaseStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
}

proptest! {
    #[test]
    fn non_increasing_capital_stage_never_registers(ceiling in 1_i128..100_000) {
        let mut invalid = plan();
        invalid.capital_stages[0].capital_ceiling_micros = ceiling;
        invalid.capital_stages[1].capital_ceiling_micros = ceiling;
        invalid = invalid.sealed(&policy());
        let mut owner = ControlledProductionRelease::new(policy()).unwrap();
        prop_assert_eq!(owner.apply(&ReleaseCommand::Register { command_id: ReleaseCommandId(id(110)), plan: Box::new(invalid), recorded_at_ns: 200 }).unwrap_err(), Error::Plan);
    }
}
