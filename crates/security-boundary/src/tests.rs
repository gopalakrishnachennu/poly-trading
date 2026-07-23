use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> SecurityPolicy {
    SecurityPolicy {
        maximum_infrastructure_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_observation_age_ns: 500,
        maximum_identity_lifetime_ns: 500,
        maximum_backoff_ns: 100,
        maximum_request_units: 1_000_000,
        maximum_requests_per_window: 100,
        maximum_identity_epochs: 16,
    }
}

fn upstream() -> InfrastructureReport {
    let covered_matrix = BackendKind::ALL
        .into_iter()
        .flat_map(|b| InfrastructureScenario::ALL.into_iter().map(move |s| (b, s)))
        .collect();
    InfrastructureReport {
        report_id: id(1),
        plan_digest: id(2),
        provider_report_digest: id(3),
        covered_matrix,
        terminal_state_digest: id(4),
        finalized_at_ns: 100,
        status: InfrastructureReportStatus::LocallyCertified,
        external_environment_certified: false,
        credential_material_created: false,
        socket_opened: false,
        external_mutation_observed: false,
        financial_authority_granted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn workload() -> WorkloadIdentityContract {
    WorkloadIdentityContract {
        cluster_digest: id(10),
        namespace_digest: id(11),
        service_account_digest: id(12),
        audience_digest: id(13),
        attestation_digest: id(14),
        maximum_lifetime_ns: 300,
        secret_value_embedded: false,
        bearer_token_embedded: false,
        strategy_access_allowed: false,
        export_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn provider_byte(v: ProviderClass) -> u8 {
    match v {
        ProviderClass::Vault => 20,
        ProviderClass::Kms => 30,
        ProviderClass::Hsm => 40,
    }
}
fn provider(v: ProviderClass) -> FakeProviderContract {
    let b = provider_byte(v);
    FakeProviderContract {
        provider: v,
        provider_subject_digest: id(b),
        primary_region_digest: id(b + 1),
        recovery_region_digest: id(b + 2),
        attestation_policy_digest: id(b + 3),
        fake_only: true,
        credential_embedded: false,
        key_material_embedded: false,
        export_allowed: false,
        network_enabled: false,
        signing_enabled: false,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn signer() -> IsolatedSignerContract {
    IsolatedSignerContract {
        process_identity_digest: id(50),
        allowed_purposes: SigningPurpose::ALL.to_vec(),
        allowed_resource_digests: vec![id(51), id(52)],
        maximum_request_units: 100_000,
        maximum_requests_per_window: 10,
        request_lifetime_ns: 100,
        dual_control_required: true,
        arbitrary_payload_allowed: false,
        transfer_allowed: false,
        withdrawal_allowed: false,
        contract_upgrade_allowed: false,
        strategy_direct_access_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> SecurityPlan {
    SecurityPlan {
        plan_id: id(60),
        infrastructure_report: upstream(),
        workload_contract: workload(),
        signer_contract: signer(),
        provider_contracts: ProviderClass::ALL.into_iter().map(provider).collect(),
        required_scenarios: SecurityScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}

fn identity(identity: u8, epoch: u64, predecessor: [u8; 32], at: i64) -> OpaqueWorkloadIdentity {
    OpaqueWorkloadIdentity {
        identity_id_digest: id(identity),
        contract_digest: workload().contract_digest,
        predecessor_identity_digest: predecessor,
        epoch,
        issued_at_ns: at,
        expires_at_ns: at + 200,
        attestation_digest: workload().attestation_digest,
        secret_material_present: false,
        token_value_present: false,
        provider_contacted: false,
        identity_digest: [0; 32],
    }
    .sealed()
}
fn disposition(s: SecurityScenario) -> SecurityDisposition {
    match s {
        SecurityScenario::IdentityIssue => SecurityDisposition::Issue,
        SecurityScenario::IdentityRotation => SecurityDisposition::Rotate,
        SecurityScenario::IdentityRevocation => SecurityDisposition::Revoke,
        SecurityScenario::ProviderOutage | SecurityScenario::RateLimit => {
            SecurityDisposition::Backoff
        }
        SecurityScenario::SignerDenial | SecurityScenario::ReplayDenied => {
            SecurityDisposition::Deny
        }
        SecurityScenario::DualControl => SecurityDisposition::Record,
        SecurityScenario::CompromiseContainment => SecurityDisposition::Reconcile,
        SecurityScenario::DisasterRecovery => SecurityDisposition::ManualRecovery,
    }
}
fn observation(
    identity_value: u8,
    scenario: SecurityScenario,
    provider_class: ProviderClass,
    identity: Option<OpaqueWorkloadIdentity>,
    at: i64,
) -> SecurityObservation {
    let contract = provider(provider_class);
    SecurityObservation {
        observation_id: id(identity_value),
        scenario,
        disposition: disposition(scenario),
        provider: provider_class,
        provider_contract_digest: contract.contract_digest,
        identity,
        security_operator_digest: if scenario == SecurityScenario::DualControl {
            id(identity_value + 1)
        } else {
            [0; 32]
        },
        operations_operator_digest: if scenario == SecurityScenario::DualControl {
            id(identity_value + 2)
        } else {
            [0; 32]
        },
        destination_region_digest: if scenario == SecurityScenario::DisasterRecovery {
            contract.recovery_region_digest
        } else {
            [0; 32]
        },
        backoff_ns: if matches!(
            scenario,
            SecurityScenario::ProviderOutage | SecurityScenario::RateLimit
        ) {
            50
        } else {
            0
        },
        observed_at_ns: at,
        automatic_retry_attempted: false,
        secret_material_observed: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        external_mutation_observed: false,
        signer_activated: false,
        observation_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> SecurityBoundaryCertification {
    let mut owner = SecurityBoundaryCertification::new(policy()).unwrap();
    owner
        .apply(&SecurityCommand::Register {
            command_id: SecurityCommandId(id(70)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}
fn apply_observation(
    owner: &mut SecurityBoundaryCertification,
    value: SecurityObservation,
    command: u8,
) -> SecurityOutcome {
    let at = value.observed_at_ns;
    owner
        .apply(&SecurityCommand::Observe {
            command_id: SecurityCommandId(id(command)),
            observation: Box::new(value),
            recorded_at_ns: at,
        })
        .unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn complete_security_campaign_certifies_without_secrets_or_authority() {
    let mut owner = registered();
    let first = identity(80, 1, [0; 32], 210);
    apply_observation(
        &mut owner,
        observation(
            81,
            SecurityScenario::IdentityIssue,
            ProviderClass::Vault,
            Some(first.clone()),
            210,
        ),
        120,
    );
    let second = identity(82, 2, first.identity_digest, 220);
    apply_observation(
        &mut owner,
        observation(
            83,
            SecurityScenario::IdentityRotation,
            ProviderClass::Kms,
            Some(second),
            220,
        ),
        121,
    );
    apply_observation(
        &mut owner,
        observation(
            84,
            SecurityScenario::ProviderOutage,
            ProviderClass::Hsm,
            None,
            230,
        ),
        122,
    );
    apply_observation(
        &mut owner,
        observation(
            85,
            SecurityScenario::SignerDenial,
            ProviderClass::Vault,
            None,
            240,
        ),
        123,
    );
    apply_observation(
        &mut owner,
        observation(
            86,
            SecurityScenario::RateLimit,
            ProviderClass::Kms,
            None,
            250,
        ),
        124,
    );
    apply_observation(
        &mut owner,
        observation(
            87,
            SecurityScenario::DualControl,
            ProviderClass::Hsm,
            None,
            260,
        ),
        125,
    );
    apply_observation(
        &mut owner,
        observation(
            90,
            SecurityScenario::ReplayDenied,
            ProviderClass::Vault,
            None,
            270,
        ),
        126,
    );
    let compromise = apply_observation(
        &mut owner,
        observation(
            91,
            SecurityScenario::CompromiseContainment,
            ProviderClass::Kms,
            None,
            280,
        ),
        127,
    );
    let requirement = match compromise.detail {
        SecurityDetail::RecoveryRequired(v) => *v,
        _ => panic!("recovery"),
    };
    let evidence = SecurityRecoveryEvidence {
        recovery_id: id(92),
        requirement_digest: requirement.requirement_digest,
        recovered_epoch: requirement.required_epoch,
        state_digest: id(93),
        observed_at_ns: 290,
        no_mutation_observed: true,
        secret_material_observed: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        identity_activated: false,
        evidence_digest: [0; 32],
    }
    .sealed();
    owner
        .apply(&SecurityCommand::Recover {
            command_id: SecurityCommandId(id(128)),
            requirement: Box::new(requirement),
            evidence,
            recorded_at_ns: 290,
        })
        .unwrap();
    apply_observation(
        &mut owner,
        observation(
            94,
            SecurityScenario::DisasterRecovery,
            ProviderClass::Hsm,
            None,
            300,
        ),
        129,
    );
    let third = identity(95, 3, [0; 32], 310);
    apply_observation(
        &mut owner,
        observation(
            96,
            SecurityScenario::IdentityIssue,
            ProviderClass::Vault,
            Some(third.clone()),
            310,
        ),
        130,
    );
    apply_observation(
        &mut owner,
        observation(
            97,
            SecurityScenario::IdentityRevocation,
            ProviderClass::Kms,
            Some(third),
            320,
        ),
        131,
    );
    let outcome = owner
        .apply(&SecurityCommand::Finalize {
            command_id: SecurityCommandId(id(132)),
            report_id: id(98),
            finalized_at_ns: 330,
            recorded_at_ns: 330,
        })
        .unwrap();
    let report = match outcome.detail {
        SecurityDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.covered_scenarios, SecurityScenario::ALL);
    assert_eq!(report.covered_providers, ProviderClass::ALL);
    assert!(
        !report.real_provider_certified
            && !report.secret_material_created
            && !report.signature_produced
    );
    assert!(!report.provider_contacted && !report.socket_opened && !report.signer_activated);
    assert!(
        !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn strategy_access_or_same_operator_dual_control_halts() {
    let mut bad = plan();
    bad.signer_contract.strategy_direct_access_allowed = true;
    bad.signer_contract = bad.signer_contract.sealed();
    bad = bad.sealed(&policy());
    let mut owner = SecurityBoundaryCertification::new(policy()).unwrap();
    assert_eq!(
        owner
            .apply(&SecurityCommand::Register {
                command_id: SecurityCommandId(id(140)),
                plan: Box::new(bad),
                recorded_at_ns: 200
            })
            .unwrap_err(),
        Error::Plan
    );
    let mut dual_owner = registered();
    let mut dual = observation(
        141,
        SecurityScenario::DualControl,
        ProviderClass::Vault,
        None,
        210,
    );
    dual.operations_operator_digest = dual.security_operator_digest;
    dual = dual.sealed();
    assert_eq!(
        dual_owner
            .apply(&SecurityCommand::Observe {
                command_id: SecurityCommandId(id(142)),
                observation: Box::new(dual),
                recorded_at_ns: 210
            })
            .unwrap_err(),
        Error::Lifecycle
    );
}

#[test]
fn automatic_retry_and_secret_observation_halt() {
    let mut owner = registered();
    let mut outage = observation(
        143,
        SecurityScenario::ProviderOutage,
        ProviderClass::Vault,
        None,
        210,
    );
    outage.automatic_retry_attempted = true;
    outage = outage.sealed();
    assert_eq!(
        owner
            .apply(&SecurityCommand::Observe {
                command_id: SecurityCommandId(id(144)),
                observation: Box::new(outage),
                recorded_at_ns: 210
            })
            .unwrap_err(),
        Error::Lifecycle
    );
    let mut secret_owner = registered();
    let mut denial = observation(
        145,
        SecurityScenario::SignerDenial,
        ProviderClass::Kms,
        None,
        210,
    );
    denial.secret_material_observed = true;
    denial = denial.sealed();
    assert_eq!(
        secret_owner
            .apply(&SecurityCommand::Observe {
                command_id: SecurityCommandId(id(146)),
                observation: Box::new(denial),
                recorded_at_ns: 210
            })
            .unwrap_err(),
        Error::Observation
    );
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
        Err(JournalBackendError::Single(JournalError::Io(
            std::io::Error::other("sync failure"),
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
    let recovery = SecurityRecovery {
        owner: SecurityBoundaryCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableSecurityBoundary::new(writer, recovery).unwrap();
    let command = SecurityCommand::Register {
        command_id: SecurityCommandId(id(160)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot().digest;
    drop(durable);
    let checkpoint = SecurityCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    let recovered = recover_segmented(&segments, policy(), Some(checkpoint)).unwrap();
    assert_eq!(recovered.owner.snapshot().digest, expected);
    let report = SecurityReport {
        report_id: id(161),
        plan_digest: id(162),
        infrastructure_report_digest: id(163),
        covered_scenarios: SecurityScenario::ALL.to_vec(),
        covered_providers: ProviderClass::ALL.to_vec(),
        final_identity_epoch: 3,
        finalized_at_ns: 300,
        status: SecurityReportStatus::LocallyCertified,
        real_provider_certified: false,
        secret_material_created: false,
        signature_produced: false,
        provider_contacted: false,
        socket_opened: false,
        signer_activated: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed();
    let report_path = dir.path().join("report.bin");
    write_report_create_new(&report_path, &report).unwrap();
    assert_eq!(read_report(&report_path).unwrap(), report);
    assert!(write_report_create_new(&report_path, &report).is_err());
    let recovery = SecurityRecovery {
        owner: SecurityBoundaryCertification::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableSecurityBoundary::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(SecurityStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(SecurityStorageError::Halted(_))
    ));
}

proptest! { #[test] fn overlong_identity_never_issues(extra in 1_i64..10_000) { let mut owner = registered(); let mut value = identity(150, 1, [0; 32], 210); value.expires_at_ns = value.issued_at_ns + workload().maximum_lifetime_ns + extra; value = value.sealed(); let result = owner.apply(&SecurityCommand::Observe { command_id: SecurityCommandId(id(151)), observation: Box::new(observation(152, SecurityScenario::IdentityIssue, ProviderClass::Vault, Some(value), 210)), recorded_at_ns: 210 }); prop_assert!(result.is_err()); prop_assert!(owner.snapshot().current_identity.is_none()); } }
