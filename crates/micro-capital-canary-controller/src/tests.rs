use super::*;
use authenticated_no_submit::{AuthReport, AuthReportStatus, AuthScenario};
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;
fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> CanaryPolicy {
    CanaryPolicy {
        maximum_auth_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_cases: 20,
    }
}
fn upstream() -> AuthReport {
    AuthReport {
        report_id: id(1),
        plan_digest: id(2),
        paper_report_digest: id(3),
        final_identity_epoch: 2,
        covered_scenarios: AuthScenario::ALL.to_vec(),
        finalized_at_ns: 100,
        status: AuthReportStatus::LocallyCertified,
        real_identity_activated: false,
        credential_material_created: false,
        signature_produced: false,
        authenticated_connection_opened: false,
        submit_capability_present: false,
        capital_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn limits() -> CanaryLimits {
    CanaryLimits {
        allocated_capital_micros: 1_000,
        capital_floor_micros: 900,
        maximum_session_loss_micros: 100,
        maximum_exposure_micros: 100,
        maximum_candidate_cost_micros: 50,
        limits_digest: [0; 32],
    }
    .sealed()
}
fn allow() -> CanaryAllowlist {
    CanaryAllowlist {
        market_digest: id(10),
        condition_digest: id(11),
        up_token_digest: id(12),
        down_token_digest: id(13),
        allowlist_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> CanaryPlan {
    CanaryPlan {
        plan_id: id(14),
        auth_report: upstream(),
        limits: limits(),
        allowlist: allow(),
        required_scenarios: CanaryScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn case(b: u8, seq: u64, s: CanaryScenario) -> CanaryCase {
    let a = allow();
    let mut c = CanaryCase {
        case_id: id(b),
        sequence: seq,
        scenario: s,
        market_digest: a.market_digest,
        condition_digest: a.condition_digest,
        up_token_digest: a.up_token_digest,
        down_token_digest: a.down_token_digest,
        complete_set_only: true,
        candidate_cost_micros: 40,
        worst_case_wealth_micros: 950,
        session_loss_micros: 50,
        exposure_micros: 40,
        disposition: CanaryDisposition::NoTrade,
        reservation_created: false,
        kill_switch_latched: false,
        cancellation_requested: false,
        ambiguous_backing_retained: false,
        external_action_observed: false,
        case_digest: [0; 32],
    };
    match s {
        CanaryScenario::EligibleNoTrade => {}
        CanaryScenario::EligibleCompleteSet => c.disposition = CanaryDisposition::CodeEligible,
        CanaryScenario::CapitalFloorDenial => c.worst_case_wealth_micros = 899,
        CanaryScenario::SessionLossDenial => c.session_loss_micros = 101,
        CanaryScenario::ExposureDenial => c.exposure_micros = 101,
        CanaryScenario::AllowlistDenial => c.market_digest = id(99),
        CanaryScenario::KillSwitch => {
            c.disposition = CanaryDisposition::RollbackRequired;
            c.kill_switch_latched = true;
            c.cancellation_requested = true;
        }
        CanaryScenario::DeadManCancel => {
            c.disposition = CanaryDisposition::RollbackRequired;
            c.cancellation_requested = true;
            c.ambiguous_backing_retained = true;
        }
        CanaryScenario::OperatorAbort | CanaryScenario::Rollback => {
            c.disposition = CanaryDisposition::RollbackRequired;
            c.cancellation_requested = true;
        }
    }
    c.sealed()
}
fn approved() -> MicroCapitalCanaryController {
    let mut o = MicroCapitalCanaryController::new(policy()).unwrap();
    o.apply(&CanaryCommand::Register {
        command_id: CanaryCommandId(id(20)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    })
    .unwrap();
    o.apply(&CanaryCommand::Approve {
        command_id: CanaryCommandId(id(21)),
        risk_operator_digest: id(22),
        operations_operator_digest: id(23),
        approved_at_ns: 201,
        recorded_at_ns: 201,
    })
    .unwrap();
    o
}
#[test]
fn all_canary_controls_certify_code_without_live_authority() {
    let mut o = approved();
    for (i, s) in CanaryScenario::ALL.into_iter().enumerate() {
        o.apply(&CanaryCommand::RecordCase {
            command_id: CanaryCommandId(id(30 + u8::try_from(i).unwrap())),
            case: case(
                50 + u8::try_from(i).unwrap(),
                u64::try_from(i + 1).unwrap(),
                s,
            ),
            recorded_at_ns: 210 + i64::try_from(i).unwrap(),
        })
        .unwrap();
    }
    let out = o
        .apply(&CanaryCommand::Finalize {
            command_id: CanaryCommandId(id(70)),
            report_id: id(71),
            finalized_at_ns: 230,
            recorded_at_ns: 230,
        })
        .unwrap();
    let r = match out.detail {
        CanaryDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert_eq!(r.covered_scenarios, CanaryScenario::ALL);
    assert!(
        !r.live_canary_complete
            && !r.legal_eligibility_confirmed
            && !r.real_capital_allocated
            && !r.credential_material_created
            && !r.signature_produced
            && !r.external_order_submitted
            && !r.capital_authority_granted
            && !r.deployment_authority_granted
            && !r.trading_authority_granted
            && !r.submission_authority_granted
    );
}
#[test]
fn same_operator_and_directional_case_halt() {
    let mut o = MicroCapitalCanaryController::new(policy()).unwrap();
    o.apply(&CanaryCommand::Register {
        command_id: CanaryCommandId(id(80)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    })
    .unwrap();
    assert_eq!(
        o.apply(&CanaryCommand::Approve {
            command_id: CanaryCommandId(id(81)),
            risk_operator_digest: id(82),
            operations_operator_digest: id(82),
            approved_at_ns: 201,
            recorded_at_ns: 201
        })
        .unwrap_err(),
        Error::Approval
    );
    let mut o = approved();
    let mut c = case(83, 1, CanaryScenario::EligibleCompleteSet);
    c.complete_set_only = false;
    c = c.sealed();
    assert_eq!(
        o.apply(&CanaryCommand::RecordCase {
            command_id: CanaryCommandId(id(84)),
            case: c,
            recorded_at_ns: 210
        })
        .unwrap_err(),
        Error::Case
    );
}
#[derive(Debug, Default)]
struct Failing {
    last: Option<u64>,
}
impl EventJournal for Failing {
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
#[allow(clippy::many_single_char_names)]
fn durable_report_and_sync_fail_closed() {
    let d = tempdir().unwrap();
    let seg = d.path().join("seg");
    let w = SegmentedJournalWriter::open(
        &seg,
        SegmentConfig {
            max_segment_bytes: 4 * 1024 * 1024,
            max_segment_records: 2,
        },
    )
    .unwrap();
    let rec = CanaryRecovery {
        owner: MicroCapitalCanaryController::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableCanaryController::new(w, rec).unwrap();
    let c = CanaryCommand::Register {
        command_id: CanaryCommandId(id(90)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&c).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let cp = CanaryCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let path = d.path().join("cp");
    write_checkpoint_create_new(&path, cp).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), cp);
    assert_eq!(
        recover_segmented(&seg, policy(), Some(cp))
            .unwrap()
            .owner
            .snapshot()
            .digest,
        expected
    );
    let r = CanaryReport {
        report_id: id(91),
        plan_digest: id(92),
        auth_report_digest: id(93),
        covered_scenarios: CanaryScenario::ALL.to_vec(),
        finalized_at_ns: 300,
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
    .sealed();
    let path = d.path().join("report");
    write_report_create_new(&path, &r).unwrap();
    assert_eq!(read_report(&path).unwrap(), r);
    let rec = CanaryRecovery {
        owner: MicroCapitalCanaryController::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut f = DurableCanaryController::new(Failing::default(), rec).unwrap();
    assert!(matches!(f.apply(&c), Err(CanaryStorageError::Journal(_))));
    assert_eq!(f.owner().snapshot().accepted_commands, 0);
}
proptest! {#[test]fn over_cost_complete_set_never_eligible(extra in 1_i128..100_000){let mut o=approved();let mut c=case(120,1,CanaryScenario::EligibleCompleteSet);c.candidate_cost_micros=limits().maximum_candidate_cost_micros+extra;c=c.sealed();prop_assert_eq!(o.apply(&CanaryCommand::RecordCase{command_id:CanaryCommandId(id(121)),case:c,recorded_at_ns:210}).unwrap_err(),Error::Case);}}
