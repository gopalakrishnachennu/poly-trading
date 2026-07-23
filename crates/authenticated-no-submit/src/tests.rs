use super::*;
use live_data_paper_certification::{PaperReport, PaperReportStatus, PaperScenario};
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;
fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> AuthPolicy {
    AuthPolicy {
        maximum_paper_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_identity_lifetime_ns: 1_000,
        maximum_fixture_age_ns: 100,
        maximum_backoff_ns: 100,
    }
}
fn upstream() -> PaperReport {
    PaperReport {
        report_id: id(1),
        plan_digest: id(2),
        campaign_report_digest: id(3),
        capture_manifest_digest: id(4),
        strategy_digest: id(5),
        record_count: 3,
        evaluation_count: 3,
        covered_scenarios: PaperScenario::ALL.to_vec(),
        finalized_at_ns: 100,
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
    .sealed()
}
fn contract() -> ObservationContract {
    ObservationContract {
        workload_digest: id(10),
        provider_digest: id(11),
        primary_region_digest: id(12),
        recovery_region_digest: id(13),
        endpoint_digest: id(14),
        allowed_events_digest: id(15),
        observation_only: true,
        credential_value_present: false,
        private_key_present: false,
        signature_capability_present: false,
        submit_endpoint_present: false,
        cancel_endpoint_present: false,
        wallet_mutation_present: false,
        arbitrary_request_allowed: false,
        submit_policy_denied: true,
        cancel_policy_denied: true,
        transfer_policy_denied: true,
        withdrawal_policy_denied: true,
        upgrade_policy_denied: true,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> AuthPlan {
    AuthPlan {
        plan_id: id(16),
        paper_report: upstream(),
        contract: contract(),
        required_scenarios: AuthScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn identity(
    b: u8,
    epoch: u64,
    pred: Option<[u8; 32]>,
    issued: i64,
    expires: i64,
) -> OpaqueIdentity {
    OpaqueIdentity {
        identity_id: id(b),
        epoch,
        predecessor_digest: pred,
        workload_digest: contract().workload_digest,
        provider_digest: contract().provider_digest,
        issued_at_ns: issued,
        expires_at_ns: expires,
        revoked: false,
        identity_digest: [0; 32],
    }
    .sealed()
}
fn fixture(
    b: u8,
    sequence: u64,
    scenario: AuthScenario,
    identity_digest: [u8; 32],
    at: i64,
) -> AuthFixture {
    let dr = scenario == AuthScenario::DisasterRecovery;
    AuthFixture {
        fixture_id: id(b),
        sequence,
        scenario,
        identity_digest,
        region_digest: if dr {
            contract().recovery_region_digest
        } else {
            contract().primary_region_digest
        },
        observed_at_ns: at,
        accepted: true,
        no_mutation_observed: true,
        automatic_retry_attempted: false,
        credential_value_present: false,
        signature_produced: false,
        authenticated_connection_opened: false,
        submission_capability_present: false,
        logical_mutation_allowed: false,
        reconciliation_complete: matches!(
            scenario,
            AuthScenario::UnknownReconciliation
                | AuthScenario::ProviderOutage
                | AuthScenario::DeadMan
        ),
        backoff_ns: 10,
        fixture_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> AuthNoSubmitCertification {
    let mut o = AuthNoSubmitCertification::new(policy()).unwrap();
    o.apply(&AuthCommand::Register {
        command_id: AuthCommandId(id(20)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    })
    .unwrap();
    o
}
#[test]
fn complete_campaign_proves_no_submit_without_real_identity() {
    let mut o = registered();
    let first = identity(21, 1, None, 201, 900);
    o.apply(&AuthCommand::Issue {
        command_id: AuthCommandId(id(22)),
        identity: first.clone(),
        recorded_at_ns: 201,
    })
    .unwrap();
    let second = identity(23, 2, Some(first.identity_digest), 202, 901);
    o.apply(&AuthCommand::Rotate {
        command_id: AuthCommandId(id(24)),
        predecessor_digest: first.identity_digest,
        identity: second.clone(),
        recorded_at_ns: 202,
    })
    .unwrap();
    for (i, s) in AuthScenario::ALL.into_iter().enumerate() {
        o.apply(&AuthCommand::RecordFixture {
            command_id: AuthCommandId(id(40 + u8::try_from(i).unwrap())),
            fixture: fixture(
                60 + u8::try_from(i).unwrap(),
                u64::try_from(i + 1).unwrap(),
                s,
                second.identity_digest,
                210 + i64::try_from(i).unwrap(),
            ),
            recorded_at_ns: 210 + i64::try_from(i).unwrap(),
        })
        .unwrap();
    }
    o.apply(&AuthCommand::Revoke {
        command_id: AuthCommandId(id(80)),
        identity_digest: second.identity_digest,
        revoked_at_ns: 230,
        recorded_at_ns: 230,
    })
    .unwrap();
    let result = o
        .apply(&AuthCommand::Finalize {
            command_id: AuthCommandId(id(81)),
            report_id: id(82),
            finalized_at_ns: 231,
            recorded_at_ns: 231,
        })
        .unwrap();
    let r = match result.detail {
        AuthDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert_eq!(r.covered_scenarios, AuthScenario::ALL);
    assert!(
        !r.real_identity_activated
            && !r.credential_material_created
            && !r.signature_produced
            && !r.authenticated_connection_opened
            && !r.submit_capability_present
            && !r.capital_authority_granted
            && !r.deployment_authority_granted
            && !r.trading_authority_granted
            && !r.submission_authority_granted
    );
}
#[test]
fn mutation_contract_and_automatic_retry_halt() {
    let mut p = plan();
    p.contract.submit_endpoint_present = true;
    p.contract = p.contract.sealed();
    p = p.sealed(&policy());
    let mut o = AuthNoSubmitCertification::new(policy()).unwrap();
    assert_eq!(
        o.apply(&AuthCommand::Register {
            command_id: AuthCommandId(id(90)),
            plan: Box::new(p),
            recorded_at_ns: 200
        })
        .unwrap_err(),
        Error::Plan
    );
    let mut o = registered();
    let i = identity(91, 1, None, 201, 900);
    o.apply(&AuthCommand::Issue {
        command_id: AuthCommandId(id(92)),
        identity: i.clone(),
        recorded_at_ns: 201,
    })
    .unwrap();
    let mut f = fixture(93, 1, AuthScenario::ProviderOutage, i.identity_digest, 210);
    f.automatic_retry_attempted = true;
    f = f.sealed();
    assert_eq!(
        o.apply(&AuthCommand::RecordFixture {
            command_id: AuthCommandId(id(94)),
            fixture: f,
            recorded_at_ns: 210
        })
        .unwrap_err(),
        Error::Fixture
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
fn durable_checkpoint_report_and_sync_are_fail_closed() {
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
    let rec = AuthRecovery {
        owner: AuthNoSubmitCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableAuthCertification::new(w, rec).unwrap();
    let c = AuthCommand::Register {
        command_id: AuthCommandId(id(100)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&c).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let cp = AuthCheckpoint {
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
    let r = AuthReport {
        report_id: id(101),
        plan_digest: id(102),
        paper_report_digest: id(103),
        final_identity_epoch: 2,
        covered_scenarios: AuthScenario::ALL.to_vec(),
        finalized_at_ns: 300,
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
    .sealed();
    let path = d.path().join("report");
    write_report_create_new(&path, &r).unwrap();
    assert_eq!(read_report(&path).unwrap(), r);
    assert!(write_report_create_new(&path, &r).is_err());
    let rec = AuthRecovery {
        owner: AuthNoSubmitCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableAuthCertification::new(Failing::default(), rec).unwrap();
    assert!(matches!(
        failing.apply(&c),
        Err(AuthStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
}
proptest! {#[test]fn overlong_identity_never_issues(extra in 1_i64..100_000){let mut o=registered();let i=identity(120,1,None,201,201+policy().maximum_identity_lifetime_ns+extra);prop_assert_eq!(o.apply(&AuthCommand::Issue{command_id:AuthCommandId(id(121)),identity:i,recorded_at_ns:201}).unwrap_err(),Error::Identity);}}
