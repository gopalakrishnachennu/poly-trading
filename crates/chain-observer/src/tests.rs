use super::*;
use market_recorder::{
    EventJournal, JournalBackendError, JournalError, SegmentConfig, SegmentedJournalWriter,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn id(v: u8) -> [u8; 32] {
    [v; 32]
}
fn policy() -> ChainPolicy {
    ChainPolicy {
        maximum_venue_report_age_ns: 10_000,
        maximum_plan_lifetime_ns: 5_000,
        maximum_observation_age_ns: 100,
        maximum_head_lag_blocks: 10,
        maximum_token_balances: 16,
        maximum_transactions: 16,
    }
}
fn venue_report() -> VenueReport {
    VenueReport {
        report_id: id(1),
        plan_digest: id(2),
        security_report_digest: id(3),
        final_epoch: 3,
        final_parameter_version: 3,
        covered_scenarios: VenueScenario::ALL.to_vec(),
        finalized_at_ns: 100,
        status: VenueReportStatus::LocallyCertified,
        live_environment_certified: false,
        credential_material_created: false,
        authenticated_session_opened: false,
        order_endpoint_present: false,
        cancel_endpoint_present: false,
        order_submitted: false,
        cancellation_submitted: false,
        deployment_authority_granted: false,
        trading_authority_granted: false,
        submission_authority_granted: false,
        report_digest: [0; 32],
    }
    .sealed()
}
fn identity() -> ChainIdentityContract {
    ChainIdentityContract {
        chain_id: 137,
        genesis_digest: id(10),
        wallet_digest: id(11),
        collateral_token_digest: id(12),
        ctf_contract_digest: id(13),
        exchange_contract_digest: id(14),
        identity_digest: [0; 32],
    }
    .sealed()
}
fn provider(provider: RpcProviderId, b: u8) -> RpcProviderContract {
    RpcProviderContract {
        provider,
        endpoint_digest: id(b),
        region_digest: id(b + 1),
        chain_id: 137,
        genesis_digest: id(10),
        read_only: true,
        credential_present: false,
        signer_present: false,
        wallet_mutation_present: false,
        transaction_submission_present: false,
        arbitrary_request_allowed: false,
        contract_digest: [0; 32],
    }
    .sealed()
}
fn plan() -> ChainPlan {
    ChainPlan {
        plan_id: id(20),
        venue_report: venue_report(),
        identity: identity(),
        providers: vec![
            provider(RpcProviderId::Primary, 21),
            provider(RpcProviderId::Secondary, 23),
            provider(RpcProviderId::Archive, 25),
        ],
        required_scenarios: ChainScenario::ALL.to_vec(),
        created_at_ns: 200,
        expires_at_ns: 4_000,
        policy_digest: [0; 32],
        plan_digest: [0; 32],
    }
    .sealed(&policy())
}
fn tx(status: TransactionStatus, block_number: Option<u64>) -> TransactionObservation {
    TransactionObservation {
        transaction_digest: id(40),
        status,
        block_number,
        effect_digest: id(41),
    }
}
fn wallet(
    collateral: i128,
    allowance: i128,
    status: TransactionStatus,
    block: Option<u64>,
) -> WalletSnapshot {
    WalletSnapshot {
        collateral_micros: collateral,
        allowance_micros: allowance,
        token_balances: vec![TokenBalance {
            token_digest: id(42),
            balance_micros: 500_000,
        }],
        transactions: vec![tx(status, block)],
        wallet_state_digest: [0; 32],
    }
    .sealed()
}
#[allow(clippy::too_many_arguments)]
fn frame(
    frame_byte: u8,
    head: u64,
    finalized: u64,
    final_hash: u8,
    collateral: i128,
    allowance: i128,
    status: TransactionStatus,
    block: Option<u64>,
    at: i64,
) -> AgreementFrame {
    let wallet = wallet(collateral, allowance, status, block);
    let contracts = plan().providers;
    let snapshots = contracts
        .into_iter()
        .enumerate()
        .map(|(offset, contract)| {
            ProviderSnapshot {
                observation_id: id(frame_byte + u8::try_from(offset).unwrap()),
                provider: contract.provider,
                provider_contract_digest: contract.contract_digest,
                chain_id: 137,
                genesis_digest: id(10),
                head_number: head,
                head_hash: id(frame_byte + 10),
                head_parent_hash: id(frame_byte + 9),
                finalized_number: finalized,
                finalized_hash: id(final_hash),
                wallet: wallet.clone(),
                event_time_ns: at - 2,
                received_time_ns: at - 1,
                observed_at_ns: at,
                observation_digest: [0; 32],
            }
            .sealed()
        })
        .collect();
    AgreementFrame {
        frame_id: id(frame_byte),
        snapshots,
        agreed_finalized_number: finalized,
        agreed_finalized_hash: id(final_hash),
        agreed_wallet_state_digest: wallet.wallet_state_digest,
        observed_at_ns: at,
        frame_digest: [0; 32],
    }
    .sealed()
}
fn registered() -> ChainObserver {
    let mut owner = ChainObserver::new(policy()).unwrap();
    owner
        .apply(&ChainCommand::Register {
            command_id: ChainCommandId(id(60)),
            plan: Box::new(plan()),
            recorded_at_ns: 200,
        })
        .unwrap();
    owner
}
fn fixture(b: u8, scenario: ChainScenario, disposition: SafeDisposition) -> FailureFixture {
    FailureFixture {
        fixture_id: id(b),
        scenario,
        disposition,
        trigger_digest: id(b + 1),
        isolated: true,
        state_contribution: false,
        rpc_mutation_observed: false,
        wallet_mutation_observed: false,
        fixture_digest: [0; 32],
    }
    .sealed()
}

#[test]
#[allow(clippy::too_many_lines)]
fn campaign_requires_three_provider_finality_and_recovers_pre_finality_reorg() {
    let mut owner = registered();
    let first = frame(
        70,
        100,
        95,
        71,
        1_000_000,
        100_000,
        TransactionStatus::Pending,
        None,
        210,
    );
    owner
        .apply(&ChainCommand::ObserveAgreement {
            command_id: ChainCommandId(id(61)),
            frame: Box::new(first),
            recorded_at_ns: 210,
        })
        .unwrap();
    let second = frame(
        80,
        102,
        97,
        81,
        1_100_000,
        120_000,
        TransactionStatus::Finalized,
        Some(96),
        220,
    );
    owner
        .apply(&ChainCommand::ObserveAgreement {
            command_id: ChainCommandId(id(62)),
            frame: Box::new(second.clone()),
            recorded_at_ns: 220,
        })
        .unwrap();
    for (b, scenario, disposition) in [
        (
            90,
            ChainScenario::ProviderDisagreement,
            SafeDisposition::Halt,
        ),
        (92, ChainScenario::StaleHead, SafeDisposition::Deny),
        (94, ChainScenario::ChainMismatch, SafeDisposition::Halt),
    ] {
        owner
            .apply(&ChainCommand::RecordFailure {
                command_id: ChainCommandId(id(b + 1)),
                fixture: fixture(b, scenario, disposition),
                recorded_at_ns: 221 + i64::from(b - 90),
            })
            .unwrap();
    }
    let outcome = owner
        .apply(&ChainCommand::ObserveReorg {
            command_id: ChainCommandId(id(100)),
            requirement_id: id(101),
            old_head_hash: second.snapshots[0].head_hash,
            new_head_hash: id(102),
            reorg_block_number: 100,
            recorded_at_ns: 230,
        })
        .unwrap();
    let requirement = match outcome.detail {
        ChainDetail::RecoveryRequired(v) => *v,
        _ => panic!("requirement"),
    };
    assert!(!owner.snapshot(230).observation_ready);
    let recovered = frame(
        110,
        103,
        98,
        111,
        1_100_000,
        120_000,
        TransactionStatus::Finalized,
        Some(96),
        240,
    );
    owner
        .apply(&ChainCommand::Recover {
            command_id: ChainCommandId(id(103)),
            requirement: Box::new(requirement),
            frame: Box::new(recovered),
            no_mutation_observed: true,
            recorded_at_ns: 240,
        })
        .unwrap();
    let result = owner
        .apply(&ChainCommand::Finalize {
            command_id: ChainCommandId(id(104)),
            report_id: id(105),
            finalized_at_ns: 241,
            recorded_at_ns: 241,
        })
        .unwrap();
    let report = match result.detail {
        ChainDetail::Finalized(v) => *v,
        _ => panic!("report"),
    };
    assert!(report.verify_digest());
    assert_eq!(report.covered_scenarios, ChainScenario::ALL);
    assert!(
        !report.live_environment_certified
            && !report.rpc_connection_opened
            && !report.credential_material_created
            && !report.wallet_access_granted
            && !report.signature_produced
            && !report.transaction_submitted
            && !report.deployment_authority_granted
            && !report.trading_authority_granted
            && !report.submission_authority_granted
    );
}

#[test]
fn incomplete_agreement_and_finalized_equivocation_halt() {
    let mut incomplete = registered();
    let mut bad = frame(
        120,
        100,
        95,
        121,
        1_000_000,
        100_000,
        TransactionStatus::Pending,
        None,
        210,
    );
    bad.snapshots.pop();
    bad = bad.sealed();
    assert_eq!(
        incomplete
            .apply(&ChainCommand::ObserveAgreement {
                command_id: ChainCommandId(id(122)),
                frame: Box::new(bad),
                recorded_at_ns: 210
            })
            .unwrap_err(),
        Error::Agreement
    );
    assert!(incomplete.is_halted());

    let mut equivocation = registered();
    equivocation
        .apply(&ChainCommand::ObserveAgreement {
            command_id: ChainCommandId(id(123)),
            frame: Box::new(frame(
                124,
                100,
                95,
                125,
                1_000_000,
                100_000,
                TransactionStatus::Pending,
                None,
                210,
            )),
            recorded_at_ns: 210,
        })
        .unwrap();
    assert_eq!(
        equivocation
            .apply(&ChainCommand::ObserveAgreement {
                command_id: ChainCommandId(id(126)),
                frame: Box::new(frame(
                    127,
                    101,
                    95,
                    128,
                    1_000_000,
                    100_000,
                    TransactionStatus::Pending,
                    None,
                    211
                )),
                recorded_at_ns: 211
            })
            .unwrap_err(),
        Error::Agreement
    );
    assert!(equivocation.is_halted());
}

#[test]
fn pending_and_mined_effects_are_not_promoted_to_finalized_balance() {
    let mut owner = registered();
    owner
        .apply(&ChainCommand::ObserveAgreement {
            command_id: ChainCommandId(id(130)),
            frame: Box::new(frame(
                131,
                100,
                95,
                132,
                1_000_000,
                100_000,
                TransactionStatus::Mined,
                Some(99),
                210,
            )),
            recorded_at_ns: 210,
        })
        .unwrap();
    let state = owner.snapshot(210);
    assert_eq!(state.spendable_collateral_micros, 1_000_000);
    assert_eq!(
        state.frame.unwrap().snapshots[0].wallet.transactions[0].status,
        TransactionStatus::Mined
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
    let recovery = ChainRecovery {
        owner: ChainObserver::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut durable = DurableChainObserver::new(writer, recovery).unwrap();
    let command = ChainCommand::Register {
        command_id: ChainCommandId(id(140)),
        plan: Box::new(plan()),
        recorded_at_ns: 200,
    };
    durable.apply(&command).unwrap();
    let expected = durable.owner().snapshot(200).digest;
    drop(durable);
    let checkpoint = ChainCheckpoint {
        sequence: 0,
        state_digest: expected,
    };
    let checkpoint_path = dir.path().join("checkpoint.bin");
    write_checkpoint_create_new(&checkpoint_path, checkpoint).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    assert_eq!(
        recover_segmented(&segments, policy(), Some(checkpoint))
            .unwrap()
            .owner
            .snapshot(200)
            .digest,
        expected
    );
    let report = ChainReport {
        report_id: id(141),
        plan_digest: id(142),
        venue_report_digest: id(143),
        final_frame_digest: id(144),
        final_finalized_number: 98,
        covered_scenarios: ChainScenario::ALL.to_vec(),
        finalized_at_ns: 300,
        status: ChainReportStatus::LocallyCertified,
        live_environment_certified: false,
        rpc_connection_opened: false,
        credential_material_created: false,
        wallet_access_granted: false,
        signature_produced: false,
        transaction_submitted: false,
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
    let recovery = ChainRecovery {
        owner: ChainObserver::new(policy()).unwrap(),
        last_sequence: None,
    };
    let mut failing = DurableChainObserver::new(FailingJournal::default(), recovery).unwrap();
    assert!(matches!(
        failing.apply(&command),
        Err(ChainStorageError::Journal(_))
    ));
    assert_eq!(failing.owner().snapshot(200).accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(ChainStorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn over_lag_provider_never_forms_agreement(extra in 11_u64..1_000) {
        let mut owner = registered();
        let mut bad = frame(150, 100, 95, 151, 1_000_000, 100_000, TransactionStatus::Pending, None, 210);
        bad.snapshots[1].head_number = 95 + extra;
        bad.snapshots[1] = bad.snapshots[1].clone().sealed();
        bad = bad.sealed();
        prop_assert_eq!(owner.apply(&ChainCommand::ObserveAgreement { command_id: ChainCommandId(id(152)), frame: Box::new(bad), recorded_at_ns: 210 }).unwrap_err(), Error::Agreement);
    }
}
