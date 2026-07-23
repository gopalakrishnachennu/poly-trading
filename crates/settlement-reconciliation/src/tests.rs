use super::*;
use accounting_ledger::{AccountingLedger, LedgerCommand, ReservationId, TokenKey};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

const LEDGER_FILL_ID: LedgerCommandId = LedgerCommandId([42; 32]);

fn config() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "0xwallet".to_owned(),
        confirmation_grace_ns: 100,
        max_intents: 16,
        max_tokens: 16,
    }
}

fn command_id(value: u8) -> ReconciliationCommandId {
    ReconciliationCommandId([value; 32])
}

fn token() -> TokenKey {
    TokenKey::new("condition", "up").expect("token")
}

fn intent() -> TradeIntent {
    TradeIntent {
        intent_id: IntentId([7; 32]),
        trade_id: "trade-1".to_owned(),
        order_id: "order-1".to_owned(),
        token: token(),
        side: Side::Buy,
        quantity_micros: 1_000_000,
        consideration_micros: 400_000,
        fee_micros: 10_000,
        ledger_command_id: LEDGER_FILL_ID,
    }
}

fn register(at: i64) -> ReconciliationCommand {
    ReconciliationCommand::RegisterIntent {
        command_id: command_id(1),
        intent: intent(),
        recorded_at_ns: at,
    }
}

fn observation(status: TradeStatus, updated_at_ns: i64) -> TradeObservation {
    TradeObservation {
        trade_id: "trade-1".to_owned(),
        order_id: "order-1".to_owned(),
        token: token(),
        side: Side::Buy,
        quantity_micros: 1_000_000,
        consideration_micros: 400_000,
        fee_micros: 10_000,
        status,
        transaction_hash: matches!(status, TradeStatus::Mined | TradeStatus::Confirmed)
            .then(|| "0xtx".to_owned()),
        matched_at_ns: 10,
        updated_at_ns,
    }
}

fn observe(command: u8, status: TradeStatus, at: i64) -> ReconciliationCommand {
    ReconciliationCommand::ObserveTrade {
        command_id: command_id(command),
        observation: observation(status, at),
        recorded_at_ns: at,
    }
}

fn funded_ledger() -> AccountingLedger {
    let mut ledger = AccountingLedger::default();
    ledger
        .apply(&LedgerCommand::FundCollateral {
            command_id: LedgerCommandId([1; 32]),
            amount_micros: 1_000_000,
            recorded_at_ns: 1,
        })
        .expect("fund");
    ledger
}

fn bought_ledger() -> AccountingLedger {
    let mut ledger = funded_ledger();
    ledger
        .apply(&LedgerCommand::ReserveCollateral {
            command_id: LedgerCommandId([2; 32]),
            reservation_id: ReservationId([9; 32]),
            amount_micros: 410_000,
            recorded_at_ns: 2,
        })
        .expect("reserve");
    ledger
        .apply(&LedgerCommand::ConfirmBuy {
            command_id: LEDGER_FILL_ID,
            reservation_id: ReservationId([9; 32]),
            token: token(),
            quantity_micros: 1_000_000,
            consideration_micros: 400_000,
            fee_micros: 10_000,
            confirmation: "confirmed-trade-1".to_owned(),
            recorded_at_ns: 3,
        })
        .expect("confirmed buy");
    ledger
}

fn chain_for(ledger: &AccountingLedger, block: u64, at: i64) -> FinalizedChainSnapshot {
    let view = ledger.reconciliation_view(&BTreeSet::new());
    FinalizedChainSnapshot {
        chain_id: 137,
        wallet: "0xwallet".to_owned(),
        block_number: block,
        block_hash: format!("0xblock{block}"),
        finalized_at_ns: at - 1,
        observed_at_ns: at,
        collateral_micros: view.collateral_micros,
        token_balances: view
            .token_balances
            .into_iter()
            .map(|balance| ChainTokenBalance {
                token: balance.token,
                balance_micros: balance.balance_micros,
            })
            .collect(),
    }
}

fn reconcile_command(
    command: u8,
    reconciler: &SettlementReconciler,
    ledger: &AccountingLedger,
    block: u64,
    at: i64,
) -> ReconciliationCommand {
    let frame = reconciler.capture_frame(ledger, chain_for(ledger, block, at), at);
    ReconciliationCommand::Reconcile {
        command_id: command_id(command),
        frame,
        recorded_at_ns: at,
    }
}

#[test]
fn documented_lifecycle_reconciles_only_after_confirmation_and_posting() {
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    reconciler.apply(&register(1)).expect("register");
    reconciler
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    reconciler
        .apply(&observe(3, TradeStatus::Mined, 20))
        .expect("mined");
    reconciler
        .apply(&observe(4, TradeStatus::Confirmed, 30))
        .expect("confirmed");
    let ledger = bought_ledger();
    let command = reconcile_command(5, &reconciler, &ledger, 100, 40);
    reconciler.apply(&command).expect("reconcile");
    let snapshot = reconciler.snapshot();
    assert_eq!(snapshot.mode, ReconciliationMode::Reconciled);
    assert!(snapshot.ready);
    assert_eq!(snapshot.confirmed_trade_count, 1);
    assert_eq!(snapshot.nonterminal_trade_count, 0);
    let gate = reconciler.risk_gate();
    assert!(gate.ready);
    assert_eq!(gate.reconciliation_digest, snapshot.digest);
    assert_eq!(gate.ledger_digest, Some(ledger.snapshot().digest));
    assert_eq!(gate.chain_block_number, Some(100));
}

#[test]
fn retry_path_can_recover_to_mined_and_confirmed() {
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    reconciler.apply(&register(1)).expect("register");
    for (id, status, at) in [
        (2, TradeStatus::Matched, 10),
        (3, TradeStatus::Retrying, 20),
        (4, TradeStatus::Mined, 30),
        (5, TradeStatus::Confirmed, 40),
    ] {
        reconciler
            .apply(&observe(id, status, at))
            .expect("legal transition");
    }
    assert!(!reconciler.is_halted());
}

#[test]
fn illegal_transition_and_terminal_mutation_are_absorbing() {
    let mut illegal = SettlementReconciler::new(config()).expect("reconciler");
    illegal.apply(&register(1)).expect("register");
    illegal
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    assert_eq!(
        illegal.apply(&observe(3, TradeStatus::Confirmed, 20)),
        Err(Error::StatusTransition)
    );
    assert!(illegal.is_halted());

    let mut terminal = SettlementReconciler::new(config()).expect("reconciler");
    terminal.apply(&register(1)).expect("register");
    terminal
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    terminal
        .apply(&observe(3, TradeStatus::Retrying, 20))
        .expect("retrying");
    terminal
        .apply(&observe(4, TradeStatus::Failed, 30))
        .expect("failed");
    assert_eq!(
        terminal.apply(&observe(5, TradeStatus::Failed, 31)),
        Err(Error::TerminalMutation)
    );
    assert!(terminal.is_halted());
}

#[test]
fn unknown_trade_changed_facts_and_missing_hash_halt() {
    let mut unknown = SettlementReconciler::new(config()).expect("reconciler");
    assert_eq!(
        unknown.apply(&observe(2, TradeStatus::Matched, 10)),
        Err(Error::UnknownTrade)
    );

    let mut facts = SettlementReconciler::new(config()).expect("reconciler");
    facts.apply(&register(1)).expect("register");
    let mut changed = observation(TradeStatus::Matched, 10);
    changed.quantity_micros += 1;
    assert_eq!(
        facts.apply(&ReconciliationCommand::ObserveTrade {
            command_id: command_id(2),
            observation: changed,
            recorded_at_ns: 10,
        }),
        Err(Error::TradeFactsChanged)
    );

    let mut missing = observation(TradeStatus::Mined, 20);
    missing.transaction_hash = None;
    assert_eq!(validate_observation(&missing), Err(Error::TransactionHash));
}

#[test]
fn nonterminal_or_failed_trade_cannot_have_a_ledger_posting() {
    for terminal in [false, true] {
        let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
        reconciler.apply(&register(1)).expect("register");
        reconciler
            .apply(&observe(2, TradeStatus::Matched, 10))
            .expect("matched");
        if terminal {
            reconciler
                .apply(&observe(3, TradeStatus::Retrying, 20))
                .expect("retrying");
            reconciler
                .apply(&observe(4, TradeStatus::Failed, 30))
                .expect("failed");
        }
        let ledger = bought_ledger();
        let command = reconcile_command(5, &reconciler, &ledger, 100, 40);
        assert_eq!(reconciler.apply(&command), Err(Error::PrematurePosting));
        assert!(reconciler.is_halted());
    }
}

#[test]
fn confirmation_grace_is_inclusive_then_expires() {
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    reconciler.apply(&register(1)).expect("register");
    reconciler
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    reconciler
        .apply(&observe(3, TradeStatus::Mined, 20))
        .expect("mined");
    reconciler
        .apply(&observe(4, TradeStatus::Confirmed, 30))
        .expect("confirmed");
    let ledger = funded_ledger();
    let boundary = reconcile_command(5, &reconciler, &ledger, 100, 130);
    reconciler.apply(&boundary).expect("inclusive boundary");
    assert_eq!(reconciler.snapshot().mode, ReconciliationMode::Pending);
    let expired = reconcile_command(6, &reconciler, &ledger, 101, 131);
    assert_eq!(reconciler.apply(&expired), Err(Error::ConfirmationExpired));
    assert!(reconciler.is_halted());
}

#[test]
fn failed_unposted_trade_can_reconcile_without_inventory_change() {
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    reconciler.apply(&register(1)).expect("register");
    reconciler
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    reconciler
        .apply(&observe(3, TradeStatus::Retrying, 20))
        .expect("retrying");
    reconciler
        .apply(&observe(4, TradeStatus::Failed, 30))
        .expect("failed");
    let ledger = funded_ledger();
    let command = reconcile_command(5, &reconciler, &ledger, 100, 40);
    reconciler.apply(&command).expect("reconcile");
    assert!(reconciler.snapshot().ready);
    assert_eq!(reconciler.snapshot().failed_trade_count, 1);
}

#[test]
fn collateral_and_token_divergence_halt_independently() {
    let ledger = funded_ledger();
    let mut cash = SettlementReconciler::new(config()).expect("reconciler");
    let mut cash_chain = chain_for(&ledger, 100, 10);
    cash_chain.collateral_micros -= 1;
    let cash_frame = cash.capture_frame(&ledger, cash_chain, 10);
    assert_eq!(
        cash.apply(&ReconciliationCommand::Reconcile {
            command_id: command_id(1),
            frame: cash_frame,
            recorded_at_ns: 10,
        }),
        Err(Error::CollateralMismatch)
    );

    let bought = bought_ledger();
    let mut tokens = SettlementReconciler::new(config()).expect("reconciler");
    let mut token_chain = chain_for(&bought, 100, 10);
    token_chain.token_balances[0].balance_micros -= 1;
    let token_frame = tokens.capture_frame(&bought, token_chain, 10);
    assert_eq!(
        tokens.apply(&ReconciliationCommand::Reconcile {
            command_id: command_id(1),
            frame: token_frame,
            recorded_at_ns: 10,
        }),
        Err(Error::TokenMismatch)
    );
}

#[test]
fn chain_and_ledger_history_regression_or_equivocation_halts() {
    let ledger = funded_ledger();
    let mut chain = SettlementReconciler::new(config()).expect("reconciler");
    let first = reconcile_command(1, &chain, &ledger, 100, 10);
    chain.apply(&first).expect("first");
    let regression = reconcile_command(2, &chain, &ledger, 99, 20);
    assert_eq!(chain.apply(&regression), Err(Error::ChainHistory));

    let mut ledger_history = SettlementReconciler::new(config()).expect("reconciler");
    let first = reconcile_command(1, &ledger_history, &ledger, 100, 10);
    ledger_history.apply(&first).expect("first");
    let mut frame = ledger_history.capture_frame(&ledger, chain_for(&ledger, 101, 20), 20);
    frame.ledger.ledger_digest = [99; 32];
    let command = ReconciliationCommand::Reconcile {
        command_id: command_id(2),
        frame,
        recorded_at_ns: 20,
    };
    assert_eq!(ledger_history.apply(&command), Err(Error::LedgerHistory));
}

#[test]
fn same_height_chain_equivocation_and_halted_ledger_are_rejected() {
    let ledger = funded_ledger();
    let mut chain_history = SettlementReconciler::new(config()).expect("reconciler");
    let first = reconcile_command(1, &chain_history, &ledger, 100, 10);
    chain_history.apply(&first).expect("first");
    let mut equivocal_chain = chain_for(&ledger, 100, 20);
    equivocal_chain.block_hash = "0xdifferent".to_owned();
    let frame = chain_history.capture_frame(&ledger, equivocal_chain, 20);
    assert_eq!(
        chain_history.apply(&ReconciliationCommand::Reconcile {
            command_id: command_id(2),
            frame,
            recorded_at_ns: 20,
        }),
        Err(Error::ChainHistory)
    );

    let mut halted_ledger = funded_ledger();
    assert!(halted_ledger
        .apply(&LedgerCommand::FundCollateral {
            command_id: LedgerCommandId([1; 32]),
            amount_micros: 2,
            recorded_at_ns: 2,
        })
        .is_err());
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    let frame = reconciler.capture_frame(&halted_ledger, chain_for(&halted_ledger, 100, 10), 10);
    assert_eq!(
        reconciler.apply(&ReconciliationCommand::Reconcile {
            command_id: command_id(1),
            frame,
            recorded_at_ns: 10,
        }),
        Err(Error::LedgerHalted)
    );
}

#[test]
fn idempotency_and_codec_are_content_bound_and_strict() {
    let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
    let command = register(1);
    assert_eq!(reconciler.apply(&command), Ok(ApplyOutcome::Applied));
    let digest = reconciler.snapshot().digest;
    assert_eq!(reconciler.apply(&command), Ok(ApplyOutcome::Duplicate));
    assert_eq!(reconciler.snapshot().digest, digest);
    let mut conflict = register(1);
    if let ReconciliationCommand::RegisterIntent { intent, .. } = &mut conflict {
        intent.order_id = "different".to_owned();
    }
    assert_eq!(reconciler.apply(&conflict), Err(Error::IdempotencyConflict));

    let bytes = encode_command(&observe(2, TradeStatus::Matched, 10)).expect("encode");
    assert_eq!(
        decode_command(&bytes),
        Ok(observe(2, TradeStatus::Matched, 10))
    );
    let mut trailing = bytes;
    trailing.extend_from_slice(b" {}");
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));
}

#[test]
fn segmented_replay_and_checkpoint_match_online_state() {
    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let recovery = ReconciliationRecovery {
        reconciler: SettlementReconciler::new(config()).expect("reconciler"),
        last_sequence: None,
    };
    let mut durable = DurableReconciler::new(writer, recovery).expect("durable");
    durable.apply(&register(1)).expect("register");
    durable
        .apply(&observe(2, TradeStatus::Matched, 10))
        .expect("matched");
    let checkpoint = ReconciliationCheckpoint {
        sequence: 1,
        reconciler_digest: durable.reconciler().snapshot().digest,
    };
    durable
        .apply(&observe(3, TradeStatus::Retrying, 20))
        .expect("retrying");
    durable
        .apply(&observe(4, TradeStatus::Failed, 30))
        .expect("failed");
    let ledger = funded_ledger();
    let command = reconcile_command(5, durable.reconciler(), &ledger, 100, 40);
    durable.apply(&command).expect("reconcile");
    let online = durable.reconciler().snapshot().digest;
    drop(durable);

    let recovered =
        recover_segmented(directory.path(), config(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.last_sequence, Some(4));
    assert_eq!(recovered.reconciler.snapshot().digest, online);
    assert!(recovered.reconciler.snapshot().ready);
}

#[test]
fn durable_integrity_halt_is_recoverable_and_rejects_later_events() {
    let directory = tempdir().expect("directory");
    let writer =
        SegmentedJournalWriter::open(directory.path(), SegmentConfig::default()).expect("writer");
    let recovery = ReconciliationRecovery {
        reconciler: SettlementReconciler::new(config()).expect("reconciler"),
        last_sequence: None,
    };
    let mut durable = DurableReconciler::new(writer, recovery).expect("durable");
    assert!(matches!(
        durable.apply(&observe(1, TradeStatus::Matched, 10)),
        Err(StorageError::Domain(Error::UnknownTrade))
    ));
    let halted_digest = durable.reconciler().snapshot().digest;
    assert!(durable.reconciler().is_halted());
    drop(durable);

    let recovered = recover_segmented(directory.path(), config(), None).expect("recover halt");
    assert!(recovered.reconciler.is_halted());
    assert_eq!(recovered.reconciler.snapshot().digest, halted_digest);
    assert_eq!(recovered.last_sequence, Some(0));
}

#[test]
fn checkpoint_is_create_new_and_checksummed() {
    let directory = tempdir().expect("directory");
    let path = directory.path().join("reconciliation.checkpoint");
    let checkpoint = ReconciliationCheckpoint {
        sequence: 4,
        reconciler_digest: [5; 32],
    };
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    assert!(write_checkpoint_create_new(&path, checkpoint).is_err());
    let mut bytes = std::fs::read(&path).expect("bytes");
    bytes[24] ^= 1;
    let corrupt = directory.path().join("corrupt.checkpoint");
    std::fs::write(&corrupt, bytes).expect("corrupt test file");
    assert!(matches!(
        read_checkpoint(corrupt),
        Err(StorageError::CheckpointChecksum)
    ));
}

#[derive(Debug, Default)]
struct FailingJournal {
    last: Option<u64>,
    fail_sync: bool,
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
        if self.fail_sync {
            Err(JournalBackendError::Single(
                market_recorder::JournalError::Io(std::io::Error::other("injected sync")),
            ))
        } else {
            Ok(())
        }
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

#[test]
fn sync_failure_never_mutates_live_reconciler_and_poisons_owner() {
    let journal = FailingJournal {
        last: None,
        fail_sync: true,
    };
    let mut durable = DurableReconciler::new(
        journal,
        ReconciliationRecovery {
            reconciler: SettlementReconciler::new(config()).expect("reconciler"),
            last_sequence: None,
        },
    )
    .expect("durable");
    assert!(matches!(
        durable.apply(&register(1)),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(durable.reconciler().snapshot().intent_count, 0);
    assert!(matches!(
        durable.apply(&register(2)),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #[test]
    fn exact_chain_balances_are_required(
        collateral in 1_i128..10_000_000,
        delta in 1_i128..1_000,
    ) {
        let mut ledger = AccountingLedger::default();
        ledger.apply(&LedgerCommand::FundCollateral {
            command_id: LedgerCommandId([1; 32]),
            amount_micros: collateral,
            recorded_at_ns: 1,
        }).expect("fund");
        let mut reconciler = SettlementReconciler::new(config()).expect("reconciler");
        let exact = reconcile_command(1, &reconciler, &ledger, 100, 10);
        reconciler.apply(&exact).expect("exact frame");
        prop_assert!(reconciler.snapshot().ready);

        let mut divergent = SettlementReconciler::new(config()).expect("reconciler");
        let mut chain = chain_for(&ledger, 100, 10);
        chain.collateral_micros = chain.collateral_micros.checked_add(delta).expect("bounded");
        let frame = divergent.capture_frame(&ledger, chain, 10);
        let command = ReconciliationCommand::Reconcile {
            command_id: command_id(1),
            frame,
            recorded_at_ns: 10,
        };
        prop_assert_eq!(divergent.apply(&command), Err(Error::CollateralMismatch));
    }
}
