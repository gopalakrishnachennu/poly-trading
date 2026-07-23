use super::*;
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use proptest::prelude::*;
use tempfile::tempdir;

fn cid(value: u8) -> CommandId {
    CommandId([value; 32])
}

fn rid(value: u8) -> ReservationId {
    ReservationId([value; 32])
}

fn lid(value: u8) -> LockId {
    LockId([value; 32])
}

fn token(value: &str) -> TokenKey {
    TokenKey::new("condition", value).expect("token")
}

fn fund(id: u8, amount: i128) -> LedgerCommand {
    LedgerCommand::FundCollateral {
        command_id: cid(id),
        amount_micros: amount,
        recorded_at_ns: i64::from(id),
    }
}

fn reserve_cash(command: u8, reservation: u8, amount: i128) -> LedgerCommand {
    LedgerCommand::ReserveCollateral {
        command_id: cid(command),
        reservation_id: rid(reservation),
        amount_micros: amount,
        recorded_at_ns: i64::from(command),
    }
}

fn buy(
    command: u8,
    reservation: u8,
    outcome: &TokenKey,
    quantity: i128,
    consideration: i128,
    fee: i128,
) -> LedgerCommand {
    LedgerCommand::ConfirmBuy {
        command_id: cid(command),
        reservation_id: rid(reservation),
        token: outcome.clone(),
        quantity_micros: quantity,
        consideration_micros: consideration,
        fee_micros: fee,
        confirmation: format!("confirmed-buy-{command}"),
        recorded_at_ns: i64::from(command),
    }
}

fn reserve_token(
    command: u8,
    reservation: u8,
    outcome: &TokenKey,
    quantity: i128,
) -> LedgerCommand {
    LedgerCommand::ReserveToken {
        command_id: cid(command),
        reservation_id: rid(reservation),
        token: outcome.clone(),
        quantity_micros: quantity,
        recorded_at_ns: i64::from(command),
    }
}

#[test]
fn rejects_unbalanced_and_incompatible_postings_atomically() {
    let mut ledger = AccountingLedger::default();
    let before = ledger.snapshot().digest;
    assert_eq!(
        ledger.post(&Transaction {
            postings: vec![
                posting(Account::CashAvailable, Asset::Collateral, 10),
                posting(Account::CapitalContributed, Asset::Collateral, -9),
            ],
        }),
        Err(Error::Unbalanced(Asset::Collateral))
    );
    assert_eq!(ledger.snapshot().digest, before);
    assert_eq!(
        ledger.post(&Transaction {
            postings: vec![
                posting(Account::CashAvailable, Asset::Outcome(token("up")), 10),
                posting(Account::External, Asset::Outcome(token("up")), -10),
            ],
        }),
        Err(Error::AccountAsset)
    );
    assert_eq!(ledger.snapshot().digest, before);
}

#[test]
fn collateral_reservation_is_backed_partial_and_single_use() {
    let mut ledger = AccountingLedger::default();
    ledger.apply(&fund(1, 1_000_000)).expect("fund");
    ledger
        .apply(&reserve_cash(2, 10, 700_000))
        .expect("reserve");
    assert_eq!(ledger.snapshot().cash_available_micros, 300_000);
    assert_eq!(ledger.snapshot().cash_reserved_micros, 700_000);
    assert_eq!(
        ledger.apply(&reserve_cash(3, 11, 300_001)),
        Err(Error::NegativeBalance)
    );
    assert!(ledger.reservation(rid(11)).is_none());

    let up = token("up");
    ledger
        .apply(&buy(4, 10, &up, 1_000_000, 400_000, 10_000))
        .expect("partial consume");
    assert_eq!(
        ledger
            .reservation(rid(10))
            .expect("reservation")
            .remaining_micros,
        290_000
    );
    ledger
        .apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(5),
            reservation_id: rid(10),
            recorded_at_ns: 5,
        })
        .expect("release");
    assert_eq!(ledger.snapshot().cash_available_micros, 590_000);
    assert_eq!(ledger.snapshot().cash_reserved_micros, 0);
    assert_eq!(
        ledger.apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(6),
            reservation_id: rid(10),
            recorded_at_ns: 6,
        }),
        Err(Error::ReservationInactive)
    );
}

#[test]
fn confirmed_buy_sell_tracks_fees_cost_and_realized_pnl() {
    let mut ledger = AccountingLedger::default();
    let up = token("up");
    ledger.apply(&fund(1, 2_000_000)).expect("fund");
    ledger
        .apply(&reserve_cash(2, 10, 1_000_000))
        .expect("cash reserve");
    ledger
        .apply(&buy(3, 10, &up, 1_000_000, 400_000, 10_000))
        .expect("buy");
    ledger
        .apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(4),
            reservation_id: rid(10),
            recorded_at_ns: 4,
        })
        .expect("release cash");
    ledger
        .apply(&reserve_token(5, 11, &up, 500_000))
        .expect("token reserve");
    ledger
        .apply(&LedgerCommand::ConfirmSell {
            command_id: cid(6),
            reservation_id: rid(11),
            quantity_micros: 500_000,
            gross_proceeds_micros: 300_000,
            fee_micros: 5_000,
            confirmation: "confirmed-sell".to_owned(),
            recorded_at_ns: 6,
        })
        .expect("sell");

    assert_eq!(
        ledger.cost_position(&up),
        CostPosition {
            quantity_micros: 500_000,
            cost_micros: 200_000,
        }
    );
    let snapshot = ledger.snapshot();
    assert_eq!(snapshot.cash_available_micros, 1_885_000);
    assert_eq!(snapshot.fees_micros, 15_000);
    assert_eq!(snapshot.realized_net_pnl_micros, 85_000);
    assert_eq!(
        ledger.balance(&Account::TokenAvailable, &Asset::Outcome(up)),
        500_000
    );
}

#[test]
fn partial_cost_allocation_rounds_against_reported_profit() {
    let position = CostPosition {
        quantity_micros: 3,
        cost_micros: 10,
    };
    assert_eq!(allocate_cost(position, 1), Ok(4));
    assert_eq!(allocate_cost(position, 3), Ok(10));
}

#[test]
fn complete_pair_is_locked_then_realized_only_after_confirmed_merge() {
    let mut ledger = AccountingLedger::default();
    let up = token("up");
    let down = token("down");
    ledger.apply(&fund(1, 2_000_000)).expect("fund");
    ledger
        .apply(&reserve_cash(2, 10, 600_000))
        .expect("up reserve");
    ledger
        .apply(&buy(3, 10, &up, 1_000_000, 400_000, 0))
        .expect("up buy");
    ledger
        .apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(4),
            reservation_id: rid(10),
            recorded_at_ns: 4,
        })
        .expect("up release");
    ledger
        .apply(&reserve_cash(5, 11, 700_000))
        .expect("down reserve");
    ledger
        .apply(&buy(6, 11, &down, 1_000_000, 500_000, 0))
        .expect("down buy");
    ledger
        .apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(7),
            reservation_id: rid(11),
            recorded_at_ns: 7,
        })
        .expect("down release");
    ledger
        .apply(&LedgerCommand::LockPair {
            command_id: cid(8),
            lock_id: lid(1),
            up: up.clone(),
            down: down.clone(),
            quantity_micros: 1_000_000,
            recorded_at_ns: 8,
        })
        .expect("lock");
    let locked = ledger.snapshot();
    assert_eq!(locked.cash_available_micros, 1_100_000);
    assert_eq!(locked.locked_pnl_micros, 100_000);
    assert_eq!(locked.realized_net_pnl_micros, 0);

    ledger
        .apply(&LedgerCommand::ConfirmMerge {
            command_id: cid(9),
            lock_id: lid(1),
            confirmation: "confirmed-merge".to_owned(),
            recorded_at_ns: 9,
        })
        .expect("merge");
    let merged = ledger.snapshot();
    assert_eq!(merged.cash_available_micros, 2_100_000);
    assert_eq!(merged.locked_pnl_micros, 0);
    assert_eq!(merged.realized_net_pnl_micros, 100_000);
    assert_eq!(merged.active_locks, 0);
}

#[test]
fn command_id_is_idempotent_and_conflict_halts() {
    let mut ledger = AccountingLedger::default();
    let original = fund(1, 1_000_000);
    assert_eq!(ledger.apply(&original), Ok(ApplyOutcome::Applied));
    let digest = ledger.snapshot().digest;
    assert_eq!(ledger.apply(&original), Ok(ApplyOutcome::Duplicate));
    assert_eq!(ledger.snapshot().digest, digest);
    assert_eq!(ledger.accepted_commands(), 1);

    assert_eq!(
        ledger.apply(&fund(1, 2_000_000)),
        Err(Error::IdempotencyConflict)
    );
    assert!(ledger.snapshot().halted);
    assert!(matches!(ledger.apply(&fund(2, 1)), Err(Error::Halted(_))));
}

#[test]
fn codec_round_trip_is_versioned_and_strict() {
    let command = reserve_token(2, 9, &token("up"), 123);
    let bytes = encode_command(&command).expect("encode");
    assert_eq!(decode_command(&bytes), Ok(command));
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    json["version"] = serde_json::json!(99);
    assert_eq!(
        decode_command(&serde_json::to_vec(&json).expect("json")),
        Err(Error::Version(99))
    );
    let mut trailing = bytes;
    trailing.extend_from_slice(b" {}");
    assert!(matches!(decode_command(&trailing), Err(Error::Json(_))));
    let unknown = br#"{"version":1,"command":{"type":"fund_collateral","value":{"command_id":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"amount_micros":1,"recorded_at_ns":1,"unexpected":true}}}"#;
    assert!(matches!(decode_command(unknown), Err(Error::Json(_))));
}

#[test]
fn segmented_replay_and_prefix_checkpoint_are_exact() {
    let directory = tempdir().expect("directory");
    let writer = SegmentedJournalWriter::open(
        directory.path(),
        SegmentConfig {
            max_segment_bytes: 1_024 * 1_024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let recovery = LedgerRecovery {
        ledger: AccountingLedger::default(),
        last_sequence: None,
    };
    let mut durable = DurableLedger::new(writer, recovery).expect("durable");
    durable.apply(&fund(1, 1_000_000)).expect("fund");
    let checkpoint = LedgerCheckpoint {
        sequence: 0,
        ledger_digest: durable.ledger().snapshot().digest,
    };
    durable
        .apply(&reserve_cash(2, 10, 400_000))
        .expect("reserve");
    durable.sync().expect("sync");
    drop(durable);

    let recovered = recover_segmented(directory.path(), Some(checkpoint)).expect("recover");
    assert_eq!(recovered.last_sequence, Some(1));
    assert_eq!(recovered.ledger.snapshot().cash_available_micros, 600_000);
    assert_eq!(recovered.ledger.snapshot().cash_reserved_micros, 400_000);

    let bad = LedgerCheckpoint {
        sequence: 0,
        ledger_digest: [99; 32],
    };
    assert!(matches!(
        recover_segmented(directory.path(), Some(bad)),
        Err(PersistenceError::CheckpointMismatch)
    ));
}

#[test]
fn checkpoint_is_create_new_checksummed_and_exact() {
    let directory = tempdir().expect("directory");
    let path = directory.path().join("ledger.checkpoint");
    let checkpoint = LedgerCheckpoint {
        sequence: 7,
        ledger_digest: [8; 32],
    };
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    assert!(write_checkpoint_create_new(&path, checkpoint).is_err());
    let mut bytes = std::fs::read(&path).expect("read bytes");
    bytes[24] ^= 1;
    let corrupt = directory.path().join("corrupt.checkpoint");
    std::fs::write(&corrupt, bytes).expect("test corruption");
    assert!(matches!(
        read_checkpoint(corrupt),
        Err(PersistenceError::CheckpointChecksum)
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
fn post_sync_failure_never_mutates_live_ledger_and_poisons_owner() {
    let journal = FailingJournal {
        last: None,
        fail_sync: true,
    };
    let mut durable = DurableLedger::new(
        journal,
        LedgerRecovery {
            ledger: AccountingLedger::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    assert!(matches!(
        durable.apply(&fund(1, 1_000_000)),
        Err(PersistenceError::Journal(_))
    ));
    assert_eq!(durable.ledger().snapshot().cash_available_micros, 0);
    assert!(matches!(
        durable.apply(&fund(2, 1_000_000)),
        Err(PersistenceError::Halted(_))
    ));
}

#[test]
fn durable_idempotency_conflict_is_recorded_then_halts_live_state() {
    let mut durable = DurableLedger::new(
        FailingJournal::default(),
        LedgerRecovery {
            ledger: AccountingLedger::default(),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&fund(1, 10)).expect("fund");
    assert!(matches!(
        durable.apply(&fund(1, 11)),
        Err(PersistenceError::Accounting(Error::IdempotencyConflict))
    ));
    assert_eq!(durable.last_sequence(), Some(1));
    assert!(durable.ledger().snapshot().halted);
    assert!(matches!(
        durable.apply(&fund(2, 1)),
        Err(PersistenceError::Accounting(Error::Halted(_)))
    ));
}

#[test]
fn reconciliation_view_contains_confirmed_assets_and_requested_commands_only() {
    let mut ledger = AccountingLedger::default();
    let up = token("up");
    ledger.apply(&fund(1, 1_000_000)).expect("fund");
    ledger
        .apply(&reserve_cash(2, 10, 500_000))
        .expect("reserve");
    ledger
        .apply(&buy(3, 10, &up, 1_000_000, 400_000, 10_000))
        .expect("buy");
    let requested = BTreeSet::from([cid(3), cid(99)]);
    let view = ledger.reconciliation_view(&requested);
    assert_eq!(view.collateral_micros, 590_000);
    assert_eq!(
        view.token_balances,
        vec![ConfirmedTokenBalance {
            token: up,
            balance_micros: 1_000_000,
        }]
    );
    assert_eq!(view.present_command_ids, BTreeSet::from([cid(3)]));
    assert_eq!(view.ledger_digest, ledger.snapshot().digest);
}

#[test]
fn confirmed_split_merge_and_redemption_preserve_inaccessible_boundaries() {
    let mut ledger = AccountingLedger::default();
    let up = token("up");
    let down = token("down");
    ledger.apply(&fund(1, 2_000_000)).expect("fund");
    ledger
        .apply(&reserve_cash(2, 10, 1_000_000))
        .expect("reserve split");
    ledger
        .apply(&LedgerCommand::ConfirmSplit {
            command_id: cid(3),
            reservation_id: rid(10),
            up: up.clone(),
            down: down.clone(),
            quantity_micros: 1_000_000,
            confirmation: "split-confirmed".into(),
            recorded_at_ns: 3,
        })
        .expect("split");
    assert_eq!(ledger.cost_position(&up).cost_micros, 500_000);
    assert_eq!(ledger.cost_position(&down).cost_micros, 500_000);
    ledger
        .apply(&LedgerCommand::LockPair {
            command_id: cid(4),
            lock_id: lid(20),
            up: up.clone(),
            down: down.clone(),
            quantity_micros: 500_000,
            recorded_at_ns: 4,
        })
        .expect("lock");
    ledger
        .apply(&LedgerCommand::ConfirmMerge {
            command_id: cid(5),
            lock_id: lid(20),
            confirmation: "merge-confirmed".into(),
            recorded_at_ns: 5,
        })
        .expect("merge");
    ledger
        .apply(&reserve_token(6, 11, &up, 500_000))
        .expect("reserve redemption");
    ledger
        .apply(&LedgerCommand::ConfirmRedemption {
            command_id: cid(7),
            reservation_id: rid(11),
            quantity_micros: 500_000,
            payout_micros: 500_000,
            confirmation: "redemption-confirmed".into(),
            recorded_at_ns: 7,
        })
        .expect("redeem");
    assert_eq!(ledger.snapshot().active_locks, 0);
    assert_eq!(ledger.cost_position(&up).quantity_micros, 0);
    assert_eq!(ledger.cost_position(&down).quantity_micros, 500_000);
    assert_eq!(ledger.snapshot().cash_available_micros, 2_000_000);
}

proptest! {
    #[test]
    fn reservation_sequences_conserve_collateral(
        capital in 1_i128..10_000_000,
        requested in 1_i128..10_000_000,
    ) {
        let amount = requested.min(capital);
        let mut ledger = AccountingLedger::default();
        ledger.apply(&fund(1, capital)).expect("fund");
        ledger.apply(&reserve_cash(2, 10, amount)).expect("reserve");
        prop_assert_eq!(
            ledger.snapshot().cash_available_micros + ledger.snapshot().cash_reserved_micros,
            capital
        );
        ledger.apply(&LedgerCommand::ReleaseReservation {
            command_id: cid(3),
            reservation_id: rid(10),
            recorded_at_ns: 3,
        }).expect("release");
        prop_assert_eq!(ledger.snapshot().cash_available_micros, capital);
        prop_assert_eq!(ledger.snapshot().cash_reserved_micros, 0);
    }
}
