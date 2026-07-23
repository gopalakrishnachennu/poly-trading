use super::*;
use accounting_ledger::{ConfirmedTokenBalance, ReservationStatus};
use market_recorder::{EventJournal, JournalBackendError, SegmentConfig, SegmentedJournalWriter};
use paired_paper_execution::{PairedExecutionCommand, PairedExecutionCommandId};
use paired_placement_policy::{PairedPolicyCommand, PairedPolicyCommandId};
use paired_settlement_runtime::PairedSettlementCommandId;
use proptest::prelude::*;
use settlement_reconciliation::{ChainTokenBalance, FinalizedChainSnapshot};
use tempfile::tempdir;

const BASE: i64 = 10_000;

const fn bytes(value: u8) -> [u8; 32] {
    [value; 32]
}

fn config() -> ReconcilerConfig {
    ReconcilerConfig {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        confirmation_grace_ns: 1_000,
        max_intents: 32,
        max_tokens: 8,
    }
}

fn token(value: &str) -> TokenKey {
    TokenKey::new("condition", value).expect("token")
}

fn run(runtime: &mut CtfTransactionRuntime, command: &CtfCommand) -> CtfOutcome {
    runtime.apply(command).expect("command")
}

fn parent(
    runtime: &mut CtfTransactionRuntime,
    id: u8,
    command: PairedSettlementCommand,
) -> PairedSettlementOutcome {
    let at = command.recorded_at_ns();
    let result = run(
        runtime,
        &CtfCommand::Parent {
            command_id: CtfCommandId(bytes(id)),
            command: Box::new(command),
            recorded_at_ns: at,
        },
    );
    match result.detail {
        CtfDetail::Parent(outcome) => *outcome,
        other => panic!("unexpected {other:?}"),
    }
}

fn chain(runtime: &CtfTransactionRuntime, block: u64, at: i64) -> FinalizedChainSnapshot {
    let view = runtime.parent().ledger_reconciliation_view();
    FinalizedChainSnapshot {
        chain_id: 137,
        wallet: "paper-wallet".into(),
        block_number: block,
        block_hash: format!("block-{block}"),
        finalized_at_ns: at,
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

fn reconcile(runtime: &mut CtfTransactionRuntime, id: u8, block: u64, at: i64) {
    let snapshot = chain(runtime, block, at);
    parent(
        runtime,
        id,
        PairedSettlementCommand::Reconcile {
            command_id: PairedSettlementCommandId(bytes(id)),
            chain: snapshot,
            evaluated_at_ns: at,
            recorded_at_ns: at,
        },
    );
}

fn funded() -> CtfTransactionRuntime {
    let mut runtime = CtfTransactionRuntime::new(config()).expect("runtime");
    parent(
        &mut runtime,
        1,
        PairedSettlementCommand::Execution {
            command_id: PairedSettlementCommandId(bytes(1)),
            command: Box::new(PairedExecutionCommand::Policy {
                command_id: PairedExecutionCommandId(bytes(1)),
                command: Box::new(PairedPolicyCommand::Fund {
                    command_id: PairedPolicyCommandId(bytes(1)),
                    amount_micros: 2_000_000,
                    recorded_at_ns: BASE,
                }),
                recorded_at_ns: BASE,
            }),
            recorded_at_ns: BASE,
        },
    );
    reconcile(&mut runtime, 2, 1, BASE + 1);
    runtime
}

fn request(
    runtime: &mut CtfTransactionRuntime,
    command: u8,
    conversion: u8,
    value: ConversionRequest,
    at: i64,
) -> CtfOutcome {
    run(
        runtime,
        &CtfCommand::Request {
            command_id: CtfCommandId(bytes(command)),
            conversion_id: ConversionId(bytes(conversion)),
            request: value,
            recorded_at_ns: at,
        },
    )
}

fn observe(
    runtime: &mut CtfTransactionRuntime,
    command: u8,
    conversion: u8,
    sequence: u64,
    event: ConversionEvent,
    at: i64,
) -> CtfOutcome {
    run(
        runtime,
        &CtfCommand::Observe {
            command_id: CtfCommandId(bytes(command)),
            observation: ConversionObservation {
                conversion_id: ConversionId(bytes(conversion)),
                source_sequence: sequence,
                event,
                event_time_ns: at,
                received_time_ns: at,
            },
            recorded_at_ns: at,
        },
    )
}

fn split_request(quantity: i128) -> ConversionRequest {
    ConversionRequest::Split {
        up: token("up"),
        down: token("down"),
        quantity_micros: quantity,
    }
}

fn confirmed_split(runtime: &mut CtfTransactionRuntime, conversion: u8, start: i64) {
    request(runtime, 3, conversion, split_request(1_000_000), start);
    observe(
        runtime,
        4,
        conversion,
        1,
        ConversionEvent::Pending {
            external_transaction_id: format!("split-{conversion}"),
        },
        start + 1,
    );
    observe(
        runtime,
        5,
        conversion,
        2,
        ConversionEvent::Confirmed {
            transaction_hash: format!("split-hash-{conversion}"),
        },
        start + 2,
    );
}

#[test]
fn split_retry_duplicate_and_confirmation_are_exact() {
    let mut runtime = funded();
    request(&mut runtime, 3, 10, split_request(1_000_000), BASE + 2);
    let risk = runtime
        .parent()
        .execution()
        .policy()
        .staging()
        .ledger_risk_view();
    assert_eq!(risk.cash_reserved_micros, 1_000_000);
    observe(
        &mut runtime,
        4,
        10,
        1,
        ConversionEvent::Pending {
            external_transaction_id: "split-10".into(),
        },
        BASE + 3,
    );
    let duplicate = observe(
        &mut runtime,
        5,
        10,
        2,
        ConversionEvent::Pending {
            external_transaction_id: "split-10".into(),
        },
        BASE + 4,
    );
    assert_eq!(duplicate.detail, CtfDetail::DuplicateSubmission);
    observe(
        &mut runtime,
        6,
        10,
        3,
        ConversionEvent::Retrying {
            reason: "relayer unavailable".into(),
        },
        BASE + 5,
    );
    observe(
        &mut runtime,
        7,
        10,
        4,
        ConversionEvent::Confirmed {
            transaction_hash: "split-hash".into(),
        },
        BASE + 6,
    );
    let view = runtime.parent().ledger_reconciliation_view();
    assert_eq!(view.collateral_micros, 1_000_000);
    assert_eq!(view.token_balances.len(), 2);
    assert!(view
        .token_balances
        .iter()
        .all(|balance| balance.balance_micros == 1_000_000));
    assert!(
        runtime
            .record(ConversionId(bytes(10)))
            .expect("record")
            .accounting_posted
    );
}

#[test]
fn split_then_merge_posts_once_and_realizes_only_after_confirmation() {
    let mut runtime = funded();
    confirmed_split(&mut runtime, 10, BASE + 2);
    reconcile(&mut runtime, 6, 2, BASE + 5);
    request(
        &mut runtime,
        7,
        11,
        ConversionRequest::Merge {
            lock_id: LockId(bytes(80)),
            up: token("up"),
            down: token("down"),
            quantity_micros: 1_000_000,
        },
        BASE + 6,
    );
    assert_eq!(
        runtime
            .parent()
            .conversion_pair_lock(LockId(bytes(80)))
            .expect("lock")
            .status,
        LockStatus::Active
    );
    observe(
        &mut runtime,
        8,
        11,
        1,
        ConversionEvent::Pending {
            external_transaction_id: "merge-11".into(),
        },
        BASE + 7,
    );
    observe(
        &mut runtime,
        9,
        11,
        2,
        ConversionEvent::Confirmed {
            transaction_hash: "merge-hash".into(),
        },
        BASE + 8,
    );
    assert_eq!(
        runtime
            .parent()
            .ledger_reconciliation_view()
            .collateral_micros,
        2_000_000
    );
    assert_eq!(runtime.snapshot().accounting_posted_count, 2);
    let duplicate = observe(
        &mut runtime,
        10,
        11,
        3,
        ConversionEvent::Confirmed {
            transaction_hash: "merge-hash".into(),
        },
        BASE + 9,
    );
    assert_eq!(duplicate.detail, CtfDetail::DuplicateTerminal);
    assert_eq!(runtime.snapshot().accounting_posted_count, 2);
}

#[test]
fn redemption_consumes_reserved_token_only_on_confirmation() {
    let mut runtime = funded();
    confirmed_split(&mut runtime, 10, BASE + 2);
    reconcile(&mut runtime, 6, 2, BASE + 5);
    request(
        &mut runtime,
        7,
        12,
        ConversionRequest::Redemption {
            token: token("up"),
            quantity_micros: 1_000_000,
            payout_micros: 1_000_000,
            resolution_digest: bytes(99),
        },
        BASE + 6,
    );
    let risk = runtime
        .parent()
        .execution()
        .policy()
        .staging()
        .ledger_risk_view();
    assert_eq!(
        risk.reserved_tokens,
        vec![ConfirmedTokenBalance {
            token: token("up"),
            balance_micros: 1_000_000,
        }]
    );
    observe(
        &mut runtime,
        8,
        12,
        1,
        ConversionEvent::Pending {
            external_transaction_id: "redeem-12".into(),
        },
        BASE + 7,
    );
    observe(
        &mut runtime,
        9,
        12,
        2,
        ConversionEvent::Confirmed {
            transaction_hash: "redeem-hash".into(),
        },
        BASE + 8,
    );
    let view = runtime.parent().ledger_reconciliation_view();
    assert_eq!(view.collateral_micros, 2_000_000);
    assert_eq!(view.token_balances.len(), 1);
    assert_eq!(view.token_balances[0].token, token("down"));
}

#[test]
fn failed_split_releases_reservation_but_failed_merge_retains_lock() {
    let mut split = funded();
    request(&mut split, 3, 10, split_request(500_000), BASE + 2);
    observe(
        &mut split,
        4,
        10,
        1,
        ConversionEvent::Failed {
            reason: "pre-submit rejection".into(),
        },
        BASE + 3,
    );
    let record = split.record(ConversionId(bytes(10))).expect("record");
    let reservation = split
        .parent()
        .execution()
        .policy()
        .staging()
        .reservation(record.reservation_id.expect("reservation"))
        .expect("reservation");
    assert_eq!(reservation.status, ReservationStatus::Released);

    let mut merge = funded();
    confirmed_split(&mut merge, 10, BASE + 2);
    reconcile(&mut merge, 6, 2, BASE + 5);
    request(
        &mut merge,
        7,
        11,
        ConversionRequest::Merge {
            lock_id: LockId(bytes(80)),
            up: token("up"),
            down: token("down"),
            quantity_micros: 1_000_000,
        },
        BASE + 6,
    );
    observe(
        &mut merge,
        8,
        11,
        1,
        ConversionEvent::Failed {
            reason: "transaction reverted".into(),
        },
        BASE + 7,
    );
    assert_eq!(
        merge
            .parent()
            .conversion_pair_lock(LockId(bytes(80)))
            .expect("lock")
            .status,
        LockStatus::Active
    );
}

#[test]
fn external_identity_and_terminal_mutation_halt_absorbingly() {
    let mut runtime = funded();
    request(&mut runtime, 3, 10, split_request(500_000), BASE + 2);
    observe(
        &mut runtime,
        4,
        10,
        1,
        ConversionEvent::Pending {
            external_transaction_id: "split-10".into(),
        },
        BASE + 3,
    );
    let result = runtime.apply(&CtfCommand::Observe {
        command_id: CtfCommandId(bytes(5)),
        observation: ConversionObservation {
            conversion_id: ConversionId(bytes(10)),
            source_sequence: 2,
            event: ConversionEvent::Pending {
                external_transaction_id: "changed".into(),
            },
            event_time_ns: BASE + 4,
            received_time_ns: BASE + 4,
        },
        recorded_at_ns: BASE + 4,
    });
    assert_eq!(result, Err(Error::ExternalIdentity));
    assert!(runtime.is_halted());
    assert!(matches!(
        runtime.apply(&CtfCommand::Request {
            command_id: CtfCommandId(bytes(6)),
            conversion_id: ConversionId(bytes(11)),
            request: split_request(1),
            recorded_at_ns: BASE + 5,
        }),
        Err(Error::Halted(_))
    ));
}

#[test]
fn terminal_confirmation_cannot_be_mutated_to_failure() {
    let mut runtime = funded();
    confirmed_split(&mut runtime, 10, BASE + 2);
    let result = runtime.apply(&CtfCommand::Observe {
        command_id: CtfCommandId(bytes(6)),
        observation: ConversionObservation {
            conversion_id: ConversionId(bytes(10)),
            source_sequence: 3,
            event: ConversionEvent::Failed {
                reason: "late contradiction".into(),
            },
            event_time_ns: BASE + 5,
            received_time_ns: BASE + 5,
        },
        recorded_at_ns: BASE + 5,
    });
    assert_eq!(result, Err(Error::Lifecycle));
    assert!(runtime.is_halted());
    assert_eq!(runtime.snapshot().accounting_posted_count, 1);
}

#[test]
fn command_and_conversion_identities_are_content_idempotent() {
    let mut runtime = funded();
    let command = CtfCommand::Request {
        command_id: CtfCommandId(bytes(3)),
        conversion_id: ConversionId(bytes(10)),
        request: split_request(500_000),
        recorded_at_ns: BASE + 2,
    };
    let first = runtime.apply(&command).expect("first request");
    assert_eq!(runtime.apply(&command), Ok(first));

    let conflicting_command = CtfCommand::Request {
        command_id: CtfCommandId(bytes(3)),
        conversion_id: ConversionId(bytes(10)),
        request: split_request(500_001),
        recorded_at_ns: BASE + 2,
    };
    assert_eq!(
        runtime.apply(&conflicting_command),
        Err(Error::IdempotencyConflict)
    );
    assert!(runtime.is_halted());

    let mut conversion_conflict = funded();
    request(
        &mut conversion_conflict,
        3,
        10,
        split_request(500_000),
        BASE + 2,
    );
    assert_eq!(
        conversion_conflict.apply(&CtfCommand::Request {
            command_id: CtfCommandId(bytes(4)),
            conversion_id: ConversionId(bytes(10)),
            request: split_request(500_000),
            recorded_at_ns: BASE + 3,
        }),
        Err(Error::Identity)
    );
    assert!(conversion_conflict.is_halted());
}

#[test]
fn external_transaction_identity_cannot_cross_requests() {
    let mut runtime = funded();
    request(&mut runtime, 3, 10, split_request(500_000), BASE + 2);
    reconcile(&mut runtime, 4, 2, BASE + 3);
    request(&mut runtime, 5, 11, split_request(500_000), BASE + 4);
    observe(
        &mut runtime,
        6,
        10,
        1,
        ConversionEvent::Pending {
            external_transaction_id: "shared-transaction".into(),
        },
        BASE + 5,
    );
    assert_eq!(
        runtime.apply(&CtfCommand::Observe {
            command_id: CtfCommandId(bytes(7)),
            observation: ConversionObservation {
                conversion_id: ConversionId(bytes(11)),
                source_sequence: 1,
                event: ConversionEvent::Pending {
                    external_transaction_id: "shared-transaction".into(),
                },
                event_time_ns: BASE + 6,
                received_time_ns: BASE + 6,
            },
            recorded_at_ns: BASE + 6,
        }),
        Err(Error::ExternalIdentity)
    );
    assert!(runtime.is_halted());
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
        Err(JournalBackendError::Single(
            market_recorder::JournalError::Io(std::io::Error::other("sync failure")),
        ))
    }

    fn last_event_sequence(&self) -> Option<u64> {
        self.last
    }
}

fn fund_command() -> CtfCommand {
    CtfCommand::Parent {
        command_id: CtfCommandId(bytes(1)),
        command: Box::new(PairedSettlementCommand::Execution {
            command_id: PairedSettlementCommandId(bytes(1)),
            command: Box::new(PairedExecutionCommand::Policy {
                command_id: PairedExecutionCommandId(bytes(1)),
                command: Box::new(PairedPolicyCommand::Fund {
                    command_id: PairedPolicyCommandId(bytes(1)),
                    amount_micros: 2_000_000,
                    recorded_at_ns: BASE,
                }),
                recorded_at_ns: BASE,
            }),
            recorded_at_ns: BASE,
        }),
        recorded_at_ns: BASE,
    }
}

#[test]
fn codec_replay_checkpoint_and_sync_failure_are_fail_closed() {
    let command = fund_command();
    assert_eq!(
        decode_command(&encode_command(&command).expect("encode")).expect("decode"),
        command
    );
    let directory = tempdir().expect("dir");
    let segments = directory.path().join("segments");
    let writer = SegmentedJournalWriter::open(
        &segments,
        SegmentConfig {
            max_segment_bytes: 256 * 1024,
            max_segment_records: 1,
        },
    )
    .expect("writer");
    let mut durable = DurableCtfRuntime::new(
        writer,
        CtfRecovery {
            runtime: CtfTransactionRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("durable");
    durable.apply(&command).expect("apply");
    let checkpoint = CtfRuntimeCheckpoint {
        sequence: 0,
        runtime_digest: durable.runtime().snapshot().digest,
    };
    let path = directory.path().join("checkpoint.bin");
    write_checkpoint_create_new(&path, checkpoint).expect("write");
    assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
    drop(durable);
    let recovered = recover_segmented(&segments, config(), Some(checkpoint)).expect("recover");
    assert_eq!(
        recovered.runtime.snapshot().digest,
        checkpoint.runtime_digest
    );

    let mut failing = DurableCtfRuntime::new(
        FailingJournal::default(),
        CtfRecovery {
            runtime: CtfTransactionRuntime::new(config()).expect("runtime"),
            last_sequence: None,
        },
    )
    .expect("failing");
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Journal(_))
    ));
    assert_eq!(failing.runtime().snapshot().accepted_commands, 0);
    assert!(matches!(
        failing.apply(&command),
        Err(StorageError::Halted(_))
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn pending_and_retrying_split_always_retain_exact_backing(quantity in 1_i128..1_000_001) {
        let mut runtime = funded();
        request(&mut runtime, 3, 10, split_request(quantity), BASE + 2);
        observe(
            &mut runtime,
            4,
            10,
            1,
            ConversionEvent::Pending { external_transaction_id: "split-property".into() },
            BASE + 3,
        );
        observe(
            &mut runtime,
            5,
            10,
            2,
            ConversionEvent::Retrying { reason: "retry".into() },
            BASE + 4,
        );
        let risk = runtime.parent().execution().policy().staging().ledger_risk_view();
        prop_assert_eq!(risk.cash_reserved_micros, quantity);
        prop_assert!(!runtime.record(ConversionId(bytes(10))).expect("record").accounting_posted);
    }
}

#[test]
fn invalid_config_is_rejected() {
    assert_eq!(
        CtfTransactionRuntime::new(ReconcilerConfig {
            chain_id: 0,
            wallet: String::new(),
            confirmation_grace_ns: 0,
            max_intents: 0,
            max_tokens: 0,
        })
        .expect_err("invalid"),
        Error::Config
    );
}
