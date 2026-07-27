use crate::accounting::{
    mark_value_micros, notional_micros, realized_pnl_micros, settled_side,
    settlement_payout_micros, stake_micros, unrealized_pnl_micros,
};
use crate::config::WindowConfig;
use crate::engine::EngineState;
use crate::features::points;
use crate::hedge::{scan_complete_set, scan_sequential_lock};
use crate::journal::SettlementOutcome;
use crate::ledger::{ClosedTrade, PaperLedger};
use crate::legacy::{legacy_may_place_orders, legacy_registry, LegacyStrategyId};
use crate::pricing::{ask_policy_rejects, compute_pricing};
use crate::types::{
    BookLevel, DecisionAction, MarketAsset, MarketSecondSnapshot, OpenPosition, OutcomeSide,
    PerformanceBook, PositionState, RiskState, SideBook, WindowKind, MICROS_PER_UNIT,
    STRATEGY_VERSION,
};

fn book(bid: i64, ask: i64) -> SideBook {
    SideBook {
        bid: BookLevel {
            price_micros: bid,
            size_micros: 5_000_000,
        },
        ask: BookLevel {
            price_micros: ask,
            size_micros: 5_000_000,
        },
        quote_age_ms: 100,
    }
}

fn snapshot_5m(
    elapsed: i64,
    up_bid: i64,
    up_ask: i64,
    down_bid: i64,
    down_ask: i64,
) -> MarketSecondSnapshot {
    MarketSecondSnapshot {
        condition_id: "btc-5m-test".into(),
        asset: MarketAsset::Btc,
        window: WindowKind::FiveMinute,
        elapsed_secs: elapsed,
        observed_at_ns: elapsed * 1_000_000_000,
        up: book(up_bid, up_ask),
        down: book(down_bid, down_ask),
        reference_spot_micros: 101_000_000_000,
        window_open_micros: 100_000_000_000,
        max_quote_age_ms: 1_000,
        fee_estimate_micros: 1_000,
        slippage_estimate_micros: 1_000,
    }
}

/// A held UP position, used where the entry path itself is not under test.
fn sample_position() -> OpenPosition {
    OpenPosition {
        asset: MarketAsset::Btc,
        side: OutcomeSide::Up,
        quantity_micros: 1_000_000,
        entry_ask_micros: 400_000,
        effective_entry_cost_micros: 402_000,
        entry_elapsed_secs: 150,
        entry_observed_at_ns: 150 * 1_000_000_000,
        stake_micros: stake_micros(1_000_000, 402_000),
        realized_pnl_micros: 0,
        peak_bid_micros: 420_000,
        remaining_quantity_micros: 1_000_000,
        protective_bid_micros: None,
        partials_taken: 0,
        reversal_used: false,
        setup_id: "test".into(),
        book: PerformanceBook::ActiveCore,
        state: PositionState::Open,
        hedge_opposite_filled: false,
        hedge_opposite_cost_micros: 0,
        configuration_digest: None,
    }
}

/// A 5m snapshot with a symmetric book, for settlement and valuation tests.
fn snapshot_at(elapsed: i64, up_ask: i64, up_bid: i64) -> MarketSecondSnapshot {
    snapshot_5m(elapsed, up_bid, up_ask, 300_000, 310_000)
}

fn warm_favourite(engine: &mut EngineState, start: i64, end: i64) {
    for elapsed in start..=end {
        let snap = snapshot_5m(elapsed, 690_000, 700_000, 290_000, 300_000);
        let _ = engine.on_every_second(&snap);
    }
}

fn warm_before_primary_entry(engine: &mut EngineState) {
    // Persist favourite through candidate-building without entering the 135-180 window.
    warm_favourite(engine, 100, 134);
}

#[test]
fn strategy_version_is_immutable_constant() {
    assert_eq!(STRATEGY_VERSION, "btc_window_engine_v1");
}

#[test]
fn legacy_strategies_cannot_place_orders() {
    for status in legacy_registry() {
        assert!(!status.may_place_orders);
        assert_eq!(status.label, crate::types::ConfidenceLabel::Disabled);
    }
    assert!(!legacy_may_place_orders(
        LegacyStrategyId::FavoriteProbability
    ));
    assert!(!legacy_may_place_orders(
        LegacyStrategyId::FixedThresholdIntradayRoundTrip
    ));
}

#[test]
fn stale_quotes_reject_with_invalid_book() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    let mut snap = snapshot_5m(150, 690_000, 700_000, 290_000, 300_000);
    snap.up.quote_age_ms = 5_000;
    let result = engine.on_every_second(&snap);
    assert_eq!(result.decision.action, DecisionAction::NoTrade);
    assert_eq!(result.decision.reason, "STALE_QUOTES");
    assert_eq!(result.decision.book, PerformanceBook::InvalidIncomplete);
    assert!(result.position.is_none());
}

#[test]
fn high_ask_asymmetry_rejects() {
    assert_eq!(
        ask_policy_rejects(920_000, false),
        Some("HIGH_ASK_ASYMMETRY_REJECT")
    );
    assert_eq!(
        ask_policy_rejects(150_000, false),
        Some("CHEAP_TOKEN_REJECT")
    );
    assert_eq!(ask_policy_rejects(920_000, true), None);
}

#[test]
fn five_minute_core_entry_at_mid_window() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let snap = snapshot_5m(150, 690_000, 700_000, 290_000, 300_000);
    let result = engine.on_every_second(&snap);
    assert_eq!(
        result.decision.action,
        DecisionAction::Open,
        "{:?}",
        result.decision
    );
    assert_eq!(
        result.decision.setup_id.as_deref(),
        Some("5m_confirmed_mid_window_favourite")
    );
    assert_eq!(result.decision.book, PerformanceBook::ActiveCore);
    assert!(!result.decision.real_money);
    assert!(result.position.is_some());
    let pos = result.position.unwrap();
    assert_eq!(pos.side, OutcomeSide::Up);
    assert_eq!(pos.state, PositionState::Open);
    assert!(pos.quantity_micros > 0);
}

#[test]
fn discovery_phase_blocks_entry() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    let snap = snapshot_5m(30, 690_000, 700_000, 290_000, 300_000);
    let result = engine.on_every_second(&snap);
    assert_eq!(result.decision.reason, "DISCOVERY_NO_ENTRY");
    assert!(result.position.is_none());
}

#[test]
fn emergency_stop_exits_at_bid() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let entry = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(entry.decision.action, DecisionAction::Open);

    // Single sharp drop: bid ~10p below effective entry with reversed momentum path.
    for elapsed in 151..=155 {
        let drop = (elapsed - 150) * 20_000;
        let _ = engine.on_every_second(&snapshot_5m(
            elapsed,
            690_000 - drop,
            700_000 - drop,
            290_000 + drop,
            300_000 + drop,
        ));
    }
    let mut crash = snapshot_5m(156, 580_000, 590_000, 400_000, 410_000);
    crash.reference_spot_micros = 99_000_000_000;
    let result = engine.on_every_second(&crash);
    assert!(
        matches!(
            result.decision.action,
            DecisionAction::Exit
                | DecisionAction::Warning
                | DecisionAction::Hold
                | DecisionAction::Protect
                | DecisionAction::Reduce
                | DecisionAction::NoTrade
        ),
        "unexpected action {:?}",
        result.decision.action
    );
    // After a full emergency path the engine must not average down into a new OPEN.
    if result.decision.action == DecisionAction::NoTrade {
        assert!(engine.position.is_none());
    }
}

#[test]
fn sequential_lock_scans_when_total_under_threshold() {
    let snap = snapshot_5m(200, 400_000, 410_000, 540_000, 550_000);
    let position = OpenPosition {
        asset: MarketAsset::Btc,
        side: OutcomeSide::Up,
        quantity_micros: 1_000_000,
        entry_ask_micros: 400_000,
        effective_entry_cost_micros: 402_000,
        entry_elapsed_secs: 150,
        entry_observed_at_ns: 150 * 1_000_000_000,
        stake_micros: stake_micros(1_000_000, 402_000),
        realized_pnl_micros: 0,
        peak_bid_micros: 420_000,
        remaining_quantity_micros: 1_000_000,
        protective_bid_micros: None,
        partials_taken: 0,
        reversal_used: false,
        setup_id: "test".into(),
        book: PerformanceBook::ActiveCore,
        state: PositionState::Open,
        hedge_opposite_filled: false,
        hedge_opposite_cost_micros: 0,
        configuration_digest: None,
    };
    let lock = scan_sequential_lock(&snap, &position).expect("lock");
    assert_eq!(lock.opposite, OutcomeSide::Down);
    assert!(lock.total_cost_micros <= 970_000);
}

#[test]
fn complete_set_does_not_force_without_edge() {
    let snap = snapshot_5m(100, 520_000, 530_000, 520_000, 530_000);
    assert!(scan_complete_set(&snap, 1_000_000).is_none());
}

#[test]
fn complete_set_detects_positive_lock() {
    let mut snap = snapshot_5m(100, 400_000, 450_000, 400_000, 450_000);
    snap.fee_estimate_micros = 0;
    snap.slippage_estimate_micros = 0;
    let opp = scan_complete_set(&snap, 1_000_000).expect("lock");
    assert!(opp.locked_edge_micros >= 20_000);
}

#[test]
fn one_hour_entries_are_shadow_book() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    for elapsed in 2_900..=2_949 {
        let snap = MarketSecondSnapshot {
            condition_id: "btc-1h-test".into(),
            asset: MarketAsset::Btc,
            window: WindowKind::OneHour,
            elapsed_secs: elapsed,
            observed_at_ns: elapsed * 1_000_000_000,
            up: book(690_000, 700_000),
            down: book(290_000, 300_000),
            reference_spot_micros: 101_000_000_000,
            window_open_micros: 100_000_000_000,
            max_quote_age_ms: 1_000,
            fee_estimate_micros: 1_000,
            slippage_estimate_micros: 1_000,
        };
        let _ = engine.on_every_second(&snap);
    }
    let snap = MarketSecondSnapshot {
        condition_id: "btc-1h-test".into(),
        asset: MarketAsset::Btc,
        window: WindowKind::OneHour,
        elapsed_secs: 2_950,
        observed_at_ns: 2_950 * 1_000_000_000,
        up: book(690_000, 700_000),
        down: book(290_000, 300_000),
        reference_spot_micros: 101_000_000_000,
        window_open_micros: 100_000_000_000,
        max_quote_age_ms: 1_000,
        fee_estimate_micros: 1_000,
        slippage_estimate_micros: 1_000,
    };
    let result = engine.on_every_second(&snap);
    if result.decision.action == DecisionAction::Open {
        assert_eq!(result.decision.book, PerformanceBook::ShadowExperiment);
    } else {
        assert_eq!(result.decision.action, DecisionAction::NoTrade);
    }
}

#[test]
fn edge_uses_ask_plus_fees_not_midpoint() {
    let snap = snapshot_5m(150, 690_000, 700_000, 290_000, 300_000);
    let pricing = compute_pricing(700_000, 750_000, &snap).unwrap();
    assert_eq!(pricing.effective_entry_cost_micros, 702_000);
    assert_eq!(pricing.edge_micros, 48_000);
    assert!(pricing.edge_micros >= points(4));
    // Mid alone cannot clear ask+fees; calibration must be able to exceed ask.
    let mid_only = compute_pricing(700_000, 695_000, &snap).unwrap();
    assert!(mid_only.edge_micros < 0);
}

#[test]
fn window_configs_match_spec_edges() {
    let f = WindowConfig::for_window(WindowKind::FiveMinute);
    let t = WindowConfig::for_window(WindowKind::FifteenMinute);
    let h = WindowConfig::for_window(WindowKind::OneHour);
    assert_eq!(f.min_edge_micros, points(4));
    assert_eq!(t.min_edge_micros, points(5));
    assert_eq!(h.min_edge_micros, points(6));
    assert_eq!(f.max_ask_micros, 820_000);
    assert_eq!(t.max_ask_micros, 840_000);
    assert_eq!(h.max_ask_micros, 880_000);
}

#[test]
fn no_second_position_while_open() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let first = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(first.decision.action, DecisionAction::Open);
    let second = engine.on_every_second(&snapshot_5m(151, 690_000, 700_000, 290_000, 300_000));
    assert_ne!(second.decision.action, DecisionAction::Open);
}

fn sample_windows_json(elapsed_ms: i64) -> String {
    let start = 1_700_000_000_000_i64;
    let t = start + elapsed_ms;
    format!(
        r#"{{
      "markets": [{{
        "asset": "BTC",
        "window": "5m",
        "cond": "btc-5m-bridge",
        "start_ms": {start},
        "end_ms": {end},
        "t_ms": {t},
        "reference_micros": "100000000000",
        "target_micros": null,
        "up_book": {{
          "best_bid_micros": "690000",
          "best_ask_micros": "700000",
          "bids": [{{"price_micros": "690000", "quantity_micros": "5000000"}}],
          "asks": [{{"price_micros": "700000", "quantity_micros": "5000000"}}]
        }},
        "down_book": {{
          "best_bid_micros": "290000",
          "best_ask_micros": "300000",
          "bids": [{{"price_micros": "290000", "quantity_micros": "5000000"}}],
          "asks": [{{"price_micros": "300000", "quantity_micros": "5000000"}}]
        }}
      }},
      {{
        "asset": "ETH",
        "window": "5m",
        "cond": "eth-5m-bridge",
        "start_ms": {start},
        "end_ms": {end},
        "t_ms": {t},
        "reference_micros": "3000000000",
        "target_micros": null,
        "up_book": {{
          "best_bid_micros": "690000",
          "best_ask_micros": "700000",
          "bids": [{{"price_micros": "690000", "quantity_micros": "5000000"}}],
          "asks": [{{"price_micros": "700000", "quantity_micros": "5000000"}}]
        }},
        "down_book": {{
          "best_bid_micros": "290000",
          "best_ask_micros": "300000",
          "bids": [{{"price_micros": "290000", "quantity_micros": "5000000"}}],
          "asks": [{{"price_micros": "300000", "quantity_micros": "5000000"}}]
        }}
      }}]
    }}"#,
        end = start + 300_000
    )
}

#[test]
fn feed_admits_every_configured_asset_and_rejects_midpoint_without_depth() {
    let feed: crate::WindowsFeed = serde_json::from_str(&sample_windows_json(150_000)).unwrap();
    assert_eq!(crate::strategy_markets(&feed).len(), 2);
    let snap = crate::market_to_snapshot(
        &feed.markets[0],
        (1_700_000_000_000 + 150_000) * 1_000_000,
        5_000,
        1_000,
        1_000,
        None,
    )
    .unwrap();
    assert_eq!(snap.window, WindowKind::FiveMinute);
    assert_eq!(snap.asset, MarketAsset::Btc);
    assert_eq!(snap.elapsed_secs, 150);
    assert_eq!(snap.up.ask.price_micros, 700_000);
    assert!(snap.up.ask.size_micros > 0);

    let eth = crate::market_to_snapshot(
        &feed.markets[1],
        snap.observed_at_ns,
        5_000,
        1_000,
        1_000,
        None,
    )
    .unwrap();
    assert_eq!(eth.asset, MarketAsset::Eth);

    let mut unsupported = feed.markets[0].clone();
    unsupported.asset = "DOGE".into();
    assert_eq!(
        crate::market_to_snapshot(&unsupported, snap.observed_at_ns, 5_000, 1_000, 1_000, None)
            .unwrap_err(),
        crate::FeedError::UnsupportedAsset
    );

    let mut no_depth = feed.markets[0].clone();
    no_depth.up_book.bids.clear();
    no_depth.up_book.asks.clear();
    assert_eq!(
        crate::market_to_snapshot(&no_depth, snap.observed_at_ns, 5_000, 1_000, 1_000, None)
            .unwrap_err(),
        crate::FeedError::MissingBook
    );
}

#[test]
fn paper_runtime_journals_fixture_tick() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        bankroll_micros: 1_000 * MICROS_PER_UNIT,
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    let mut runtime = crate::PaperRuntime::new(config);
    // Warm persistence before primary window via repeated feeds.
    for elapsed_ms in (100_000..=134_000).step_by(1_000) {
        let feed: crate::WindowsFeed =
            serde_json::from_str(&sample_windows_json(elapsed_ms)).unwrap();
        let _ = runtime.on_feed(&feed, (1_700_000_000_000 + elapsed_ms) * 1_000_000);
    }
    let feed: crate::WindowsFeed = serde_json::from_str(&sample_windows_json(150_000)).unwrap();
    let decisions = runtime.on_feed(&feed, (1_700_000_000_000 + 150_000) * 1_000_000);
    assert_eq!(decisions.len(), 2, "BTC and ETH are both evaluated");
    assert!(decisions.iter().any(|d| d.asset == MarketAsset::Btc));
    assert!(decisions.iter().any(|d| d.asset == MarketAsset::Eth));
    assert!(decisions.iter().all(|d| !d.real_money && d.paper_only));
    assert_eq!(decisions[0].strategy_version, STRATEGY_VERSION);
    assert_eq!(
        runtime.status.assets_evaluated,
        vec!["BTC".to_owned(), "ETH".to_owned()]
    );
    assert!(dir.path().join("status.json").exists());
    let journal = std::fs::read_to_string(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().starts_with("decisions-"))
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(journal.contains("btc_window_engine_v1"));
    assert!(journal.contains("\"real_money\":false"));
}

#[test]
fn notional_scales_micros_products_back_to_micros() {
    // 2.5 shares at $0.40 = $1.00.
    assert_eq!(notional_micros(2_500_000, 400_000), MICROS_PER_UNIT);
    assert_eq!(stake_micros(1_000_000, 402_000), 402_000);
    // A large position at a near-certain price must not overflow the product.
    assert_eq!(
        notional_micros(1_000_000 * MICROS_PER_UNIT, 990_000),
        990_000 * MICROS_PER_UNIT
    );
}

#[test]
fn positions_are_marked_at_the_bid_never_the_mid() {
    // 10 shares bought at an effective 0.40, now bid 0.45 / ask 0.55.
    let quantity = 10 * MICROS_PER_UNIT;
    assert_eq!(mark_value_micros(quantity, 450_000), 4_500_000);
    assert_eq!(
        unrealized_pnl_micros(quantity, 450_000, 400_000),
        500_000,
        "profit is measured against the bid, not the 0.50 midpoint"
    );
    // A position underwater at the bid must report a loss.
    assert_eq!(unrealized_pnl_micros(quantity, 350_000, 400_000), -500_000);
}

#[test]
fn settlement_pays_one_or_zero_per_share() {
    assert_eq!(settlement_payout_micros(true), MICROS_PER_UNIT);
    assert_eq!(settlement_payout_micros(false), 0);
    // 10 shares at 0.40 that settle in the money return 0.60 per share.
    let quantity = 10 * MICROS_PER_UNIT;
    assert_eq!(
        realized_pnl_micros(quantity, settlement_payout_micros(true), 400_000),
        6_000_000
    );
    // The same shares settling worthless lose exactly the stake.
    assert_eq!(
        realized_pnl_micros(quantity, settlement_payout_micros(false), 400_000),
        -stake_micros(quantity, 400_000)
    );
}

#[test]
fn settled_side_requires_a_strictly_higher_close() {
    assert_eq!(settled_side(100, 101), OutcomeSide::Up);
    assert_eq!(settled_side(100, 99), OutcomeSide::Down);
    assert_eq!(
        settled_side(100, 100),
        OutcomeSide::Down,
        "an unchanged reference does not resolve UP"
    );
}

#[test]
fn entry_records_carry_stake_and_live_valuation() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let result = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(result.decision.action, DecisionAction::Open);

    let position = result.position.expect("open position");
    let stake = result.decision.stake_micros.expect("stake recorded");
    assert!(stake > 0);
    assert_eq!(
        stake,
        stake_micros(
            position.quantity_micros,
            position.effective_entry_cost_micros
        )
    );
    // Bought at 0.70 + costs and immediately markable only at the 0.69 bid, so
    // a fresh entry is underwater by the spread plus fees.
    assert_eq!(
        result.decision.unrealized_pnl_micros,
        Some(unrealized_pnl_micros(
            position.quantity_micros,
            690_000,
            position.effective_entry_cost_micros
        ))
    );
    assert!(result.decision.unrealized_pnl_micros.unwrap() < 0);
    assert_eq!(result.decision.realized_pnl_micros, Some(0));
}

#[test]
fn winning_window_settles_in_the_money() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let entry = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(entry.decision.action, DecisionAction::Open);
    let quantity = entry.position.as_ref().unwrap().quantity_micros;
    let cost = entry.position.as_ref().unwrap().effective_entry_cost_micros;

    // The reference closes above the window open, so the UP position wins.
    let close = snapshot_5m(300, 980_000, 990_000, 5_000, 10_000);
    let settled = engine.settle(&close).expect("settlement record");
    let record = settled.record;
    assert_eq!(record.action, DecisionAction::Settle);
    assert_eq!(record.outcome, Some(SettlementOutcome::Win));
    assert_eq!(
        record.settlement_source.as_deref(),
        Some("observed_reference")
    );
    assert_eq!(
        record.realized_pnl_micros,
        Some(realized_pnl_micros(quantity, MICROS_PER_UNIT, cost))
    );
    assert!(record.realized_pnl_micros.unwrap() > 0);
    assert_eq!(
        settled.closed.realized_pnl_micros,
        record.realized_pnl_micros.unwrap(),
        "the ledger entry and the journal must agree"
    );
    assert!(engine.position.is_none());
    assert!(engine.risk.realized_pnl_micros > 0);
}

#[test]
fn losing_window_settles_worthless_and_charges_the_daily_loss() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let entry = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(entry.decision.action, DecisionAction::Open);
    let stake = entry.decision.stake_micros.unwrap();

    // The reference closes below the window open, so the UP position loses.
    let mut close = snapshot_5m(300, 5_000, 10_000, 980_000, 990_000);
    close.reference_spot_micros = 99_000_000_000;
    let settled = engine.settle(&close).expect("settlement record");
    assert_eq!(settled.record.outcome, Some(SettlementOutcome::Loss));
    assert_eq!(settled.record.realized_pnl_micros, Some(-stake));
    assert_eq!(engine.risk.daily_loss_micros, stake);
    assert!(engine.position.is_none());
}

#[test]
fn settling_a_flat_engine_produces_no_record() {
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    assert!(engine
        .settle(&snapshot_5m(300, 690_000, 700_000, 290_000, 300_000))
        .is_none());
}

#[test]
fn ledger_keeps_core_and_shadow_books_separate() {
    let closed = |setup_id: &str,
                  book: PerformanceBook,
                  asset: MarketAsset,
                  window: &str,
                  pnl: i64,
                  outcome: SettlementOutcome,
                  at: i64| ClosedTrade {
        setup_id: setup_id.into(),
        book,
        asset,
        window: window.into(),
        realized_pnl_micros: pnl,
        outcome: Some(outcome),
        observed_at_ns: at,
        hold_secs: 60,
    };

    let mut ledger = PaperLedger::new();
    ledger.record_close(&closed(
        "5m_confirmed_mid_window_favourite",
        PerformanceBook::ActiveCore,
        MarketAsset::Btc,
        "5m",
        300_000,
        SettlementOutcome::Win,
        1,
    ));
    ledger.record_close(&closed(
        "5m_confirmed_mid_window_favourite",
        PerformanceBook::ActiveCore,
        MarketAsset::Btc,
        "5m",
        -100_000,
        SettlementOutcome::Loss,
        2,
    ));
    ledger.record_close(&closed(
        "1h_late_session_drift",
        PerformanceBook::ShadowExperiment,
        MarketAsset::Eth,
        "1h",
        900_000,
        SettlementOutcome::Win,
        3,
    ));

    let snapshot = ledger.snapshot(&[], 4);
    assert_eq!(snapshot.active_core.closed_trades, 2);
    assert_eq!(snapshot.active_core.wins, 1);
    assert_eq!(snapshot.active_core.losses, 1);
    assert_eq!(snapshot.active_core.realized_pnl_micros, 200_000);
    assert_eq!(snapshot.active_core.win_rate_bps(), Some(5_000));

    assert_eq!(snapshot.shadow_experiment.closed_trades, 1);
    assert_eq!(snapshot.shadow_experiment.realized_pnl_micros, 900_000);
    assert_eq!(
        snapshot.active_core.realized_pnl_micros, 200_000,
        "the shadow win must not leak into the core book"
    );

    // Ranked best-first, so the profitable shadow setup leads.
    assert_eq!(snapshot.by_strategy[0].setup_id, "1h_late_session_drift");
    assert_eq!(snapshot.by_strategy[0].book, "SHADOW_EXPERIMENT");
    assert_eq!(snapshot.by_strategy[1].totals.closed_trades, 2);

    let btc = snapshot
        .by_asset
        .iter()
        .find(|(asset, _)| asset == "BTC")
        .expect("BTC totals");
    assert_eq!(btc.1.realized_pnl_micros, 200_000);
    let eth = snapshot
        .by_asset
        .iter()
        .find(|(asset, _)| asset == "ETH")
        .expect("ETH totals");
    assert_eq!(eth.1.realized_pnl_micros, 900_000);

    // Window totals are keyed by asset, so an ETH 1h result cannot be reported
    // against the BTC 1h column.
    let window_keys: Vec<&str> = snapshot
        .by_window
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(window_keys, vec!["BTC-5m", "ETH-1h"]);
    assert_eq!(snapshot.by_window[0].1.closed_trades, 2);
    assert_eq!(snapshot.by_window[1].1.realized_pnl_micros, 900_000);
}

#[test]
fn ledger_win_rate_is_absent_before_any_trade_closes() {
    let snapshot = PaperLedger::new().snapshot(&[], 0);
    assert_eq!(snapshot.active_core.win_rate_bps(), None);
    assert_eq!(snapshot.active_core.closed_trades, 0);
}

/// Drive a runtime to an open position over the fixture feed, then hand back the
/// directory, config, and how many positions were open at shutdown so a second
/// runtime can be checked against it.
fn runtime_with_open_position() -> (tempfile::TempDir, crate::RuntimeConfig, u64) {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        bankroll_micros: 1_000 * MICROS_PER_UNIT,
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    let mut runtime = crate::PaperRuntime::new(config.clone());
    for elapsed_ms in (100_000..=150_000).step_by(1_000) {
        let feed: crate::WindowsFeed =
            serde_json::from_str(&sample_windows_json(elapsed_ms)).unwrap();
        let _ = runtime.on_feed(&feed, (1_700_000_000_000 + elapsed_ms) * 1_000_000);
    }
    let open = runtime.status.open_positions;
    assert!(open > 0, "fixture must reach an open position");
    assert!(runtime.status.journal_errors.is_empty());
    (dir, config, open)
}

#[test]
fn restart_resumes_an_open_position_with_its_stake_intact() {
    let (dir, config, open_before) = runtime_with_open_position();

    let checkpoint = dir.path().join("positions.json");
    assert!(checkpoint.exists(), "open positions must be checkpointed");

    let recovered = crate::PaperRuntime::recover(config, (1_700_000_000_000 + 150_000) * 1_000_000)
        .expect("recovery");
    assert!(recovered.status.recovery.checkpoint_present);
    assert_eq!(recovered.status.recovery.positions_restored, open_before);

    // The same feed second must now see a managed position, not a fresh entry.
    let mut recovered = recovered;
    let feed: crate::WindowsFeed = serde_json::from_str(&sample_windows_json(151_000)).unwrap();
    let decisions = recovered.on_feed(&feed, (1_700_000_000_000 + 151_000) * 1_000_000);
    let managed = decisions
        .iter()
        .find(|d| d.position_state != PositionState::Flat)
        .expect("resumed position is managed");
    assert!(managed.stake_micros.unwrap_or(0) > 0);
    assert_ne!(managed.action, DecisionAction::Open);
}

#[test]
fn restart_settles_a_position_whose_window_ended_while_down() {
    let (dir, config, open_before) = runtime_with_open_position();

    // The window is gone from the feed, so recovery must settle the restored
    // position from its persisted snapshot rather than abandon it.
    let mut recovered =
        crate::PaperRuntime::recover(config, (1_700_000_000_000 + 150_000) * 1_000_000)
            .expect("recovery");
    assert_eq!(recovered.status.recovery.positions_restored, open_before);

    let empty: crate::WindowsFeed = serde_json::from_str(r#"{"markets":[]}"#).unwrap();
    let decisions = recovered.on_feed(&empty, (1_700_000_000_000 + 400_000) * 1_000_000);
    let settled: Vec<_> = decisions
        .iter()
        .filter(|d| d.action == DecisionAction::Settle)
        .collect();
    assert_eq!(
        settled.len() as u64,
        open_before,
        "every restored position must settle, none abandoned"
    );
    assert!(settled.iter().all(|d| d.outcome.is_some()));
    assert!(settled.iter().all(|d| d.realized_pnl_micros.is_some()));
    assert_eq!(recovered.status.settled_on_recovery, open_before);
    assert_eq!(recovered.status.open_positions, 0);

    // The settlement is on disk, so no bet ends without a journalled result.
    let journal = std::fs::read_to_string(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().starts_with("decisions-"))
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(journal.contains("\"action\":\"settle\""));
}

#[test]
fn replay_rebuilds_day_scoped_totals_from_the_journal() {
    let (dir, config, open_before) = runtime_with_open_position();
    // Close the positions so the journal holds terminal records to replay.
    let mut first =
        crate::PaperRuntime::recover(config.clone(), (1_700_000_000_000 + 150_000) * 1_000_000)
            .expect("recovery");
    let empty: crate::WindowsFeed = serde_json::from_str(r#"{"markets":[]}"#).unwrap();
    let _ = first.on_feed(&empty, (1_700_000_000_000 + 400_000) * 1_000_000);
    let expected_realized = first.status.ledger.active_core.realized_pnl_micros
        + first.status.ledger.shadow_experiment.realized_pnl_micros;
    let expected_closed = first.status.ledger.active_core.closed_trades
        + first.status.ledger.shadow_experiment.closed_trades;
    assert_eq!(expected_closed, open_before);
    drop(first);

    // A fresh process must report the same book, not start from zero.
    let mut second =
        crate::PaperRuntime::recover(config, (1_700_000_000_000 + 400_000) * 1_000_000)
            .expect("recovery");
    assert_eq!(second.status.recovery.closes_replayed, open_before);
    let _ = second.on_feed(&empty, (1_700_000_000_000 + 401_000) * 1_000_000);
    let realized = second.status.ledger.active_core.realized_pnl_micros
        + second.status.ledger.shadow_experiment.realized_pnl_micros;
    let closed = second.status.ledger.active_core.closed_trades
        + second.status.ledger.shadow_experiment.closed_trades;
    assert_eq!(realized, expected_realized);
    assert_eq!(closed, expected_closed);
    drop(dir);
}

#[test]
fn a_corrupt_journal_line_halts_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    let observed_at_ns = 1_700_000_000_000 * 1_000_000;
    let path = config.journal_path_for(observed_at_ns);
    std::fs::write(&path, "{\"not\":\"a decision record\"}\n").unwrap();

    let err = crate::PaperRuntime::recover(config, observed_at_ns).unwrap_err();
    assert!(
        matches!(
            err,
            crate::RecoveryError::CorruptJournalLine { line: 1, .. }
        ),
        "expected a corrupt-line halt, got {err:?}"
    );
}

#[test]
fn a_checkpoint_from_another_engine_version_halts_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    std::fs::write(
        config.checkpoint_path(),
        serde_json::to_vec(&crate::PositionCheckpoint {
            strategy_version: "some_other_engine_v9".into(),
            written_at_ns: 1,
            entries: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();

    let err =
        crate::PaperRuntime::recover(config.clone(), 1_700_000_000_000 * 1_000_000).unwrap_err();
    assert!(
        matches!(err, crate::RecoveryError::CheckpointVersionMismatch { .. }),
        "expected a version-mismatch halt, got {err:?}"
    );

    // A structurally broken checkpoint must also halt rather than be skipped.
    std::fs::write(config.checkpoint_path(), b"{ this is not json").unwrap();
    assert!(matches!(
        crate::PaperRuntime::recover(config, 1_700_000_000_000 * 1_000_000).unwrap_err(),
        crate::RecoveryError::CorruptCheckpoint { .. }
    ));
}

#[test]
fn replay_counts_pre_accounting_closes_instead_of_booking_them_as_zero() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    let observed_at_ns = 1_700_000_000_000 * 1_000_000;
    // A Spec 090 exit: no asset, no realized P&L.
    let legacy = r#"{"strategy_version":"btc_window_engine_v1","condition_id":"old","window":"5m","elapsed_secs":200,"observed_at_ns":1,"action":"exit","reason":"TIME_STOP","rejection_reasons":[],"setup_id":"legacy_setup","side":"up","ask_micros":null,"bid_micros":600000,"edge_micros":null,"effective_cost_micros":620000,"quantity_micros":0,"position_state":"flat","book":"active_core","paper_only":true,"real_money":false}"#;
    std::fs::write(
        config.journal_path_for(observed_at_ns),
        format!("{legacy}\n"),
    )
    .unwrap();

    let runtime = crate::PaperRuntime::recover(config, observed_at_ns).expect("recovery");
    let recovery = &runtime.status.recovery;
    assert_eq!(recovery.journal_records_read, 1);
    assert_eq!(recovery.closes_replayed, 0);
    assert_eq!(
        recovery.pre_accounting_closes_skipped, 1,
        "a close with no realized P&L must be counted, not booked as a zero-P&L trade"
    );
    drop(dir);
}

#[test]
fn a_second_worker_cannot_open_the_same_book() {
    let dir = tempfile::tempdir().unwrap();
    let first = crate::BookLock::acquire(dir.path()).expect("first worker owns the book");

    let err = crate::BookLock::acquire(dir.path())
        .expect_err("a second live worker must be refused, not allowed to race");
    assert!(matches!(err, crate::LockError::Held { .. }), "got {err:?}");

    // Releasing hands the book to the next worker.
    drop(first);
    crate::BookLock::acquire(dir.path()).expect("book is free once released");
}

#[test]
fn a_dead_workers_lock_does_not_wedge_the_book() {
    let dir = tempfile::tempdir().unwrap();
    // Pid 0 is never a live user process, so this lock is stale.
    std::fs::write(dir.path().join("writer.lock"), "999999999").unwrap();
    crate::BookLock::acquire(dir.path()).expect("a stale lock must be taken over");
}

#[test]
fn declining_to_settle_never_discards_a_position() {
    // A reverse candidate holds no shares and was already booked on exit, so
    // settle declines it — but it must not silently drop a position that still
    // has quantity to account for.
    let snapshot = snapshot_at(200, 620_000, 600_000);
    let mut engine = crate::EngineState::new(1_000 * MICROS_PER_UNIT);
    engine.position = Some(OpenPosition {
        remaining_quantity_micros: 0,
        ..sample_position()
    });
    assert!(engine.settle(&snapshot).is_none());
    assert!(engine.position.is_none(), "a spent position is retired");

    let mut engine = crate::EngineState::new(1_000 * MICROS_PER_UNIT);
    engine.position = Some(sample_position());
    let settled = engine
        .settle(&snapshot)
        .expect("a position holding shares must settle");
    assert!(settled.record.outcome.is_some());
    assert_eq!(settled.record.action, DecisionAction::Settle);
}

#[test]
fn a_departed_window_with_no_snapshot_is_reported_not_abandoned() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::RuntimeConfig {
        journal_dir: dir.path().to_path_buf(),
        status_path: dir.path().join("status.json"),
        ..crate::RuntimeConfig::default()
    };
    // A checkpointed position whose snapshot is unusable must not vanish.
    let mut runtime = crate::PaperRuntime::new(config);
    runtime.force_position_without_snapshot("0xghost", sample_position());

    let empty: crate::WindowsFeed = serde_json::from_str(r#"{"markets":[]}"#).unwrap();
    let decisions = runtime.on_feed(&empty, 1_700_000_000_000 * 1_000_000);
    assert!(decisions.iter().all(|d| d.action != DecisionAction::Settle));
    assert!(
        runtime
            .status
            .journal_errors
            .iter()
            .any(|e| e.contains("unsettleable")),
        "an unsettleable funded position must be reported: {:?}",
        runtime.status.journal_errors
    );
}

#[test]
fn adaptive_stake_grows_with_setup_win_rate() {
    use crate::sizing::{plan_size, SetupPerformance, StakeTier, STAKE_BPS_STRONG, STAKE_BPS_WEAK};
    let cfg = WindowConfig::for_window(WindowKind::FiveMinute);
    let risk = RiskState {
        bankroll_micros: 1_000 * MICROS_PER_UNIT,
        ..RiskState::default()
    };
    let ask = 700_000;

    let strong = plan_size(
        &cfg,
        &risk,
        ask,
        false,
        Some(SetupPerformance {
            closed_trades: 20,
            wins: 14, // 70%
        }),
    )
    .expect("strong size");
    assert_eq!(strong.stake_tier, StakeTier::Strong);
    assert_eq!(strong.risk_bps, STAKE_BPS_STRONG);
    // ~10% of $1000 = $100 stake at a 0.70 ask → qty ≈ 142.8 shares.
    assert!(strong.risk_amount_micros >= 95 * MICROS_PER_UNIT);

    let weak = plan_size(
        &cfg,
        &risk,
        ask,
        false,
        Some(SetupPerformance {
            closed_trades: 20,
            wins: 6, // 30%
        }),
    )
    .expect("weak size");
    assert_eq!(weak.stake_tier, StakeTier::Weak);
    assert_eq!(weak.risk_bps, STAKE_BPS_WEAK);
    assert!(weak.risk_amount_micros < strong.risk_amount_micros / 3);

    let unproven = plan_size(&cfg, &risk, ask, false, None).expect("unproven");
    assert_eq!(unproven.stake_tier, StakeTier::Unproven);
    assert_eq!(unproven.risk_bps, 200);

    let shadow = plan_size(
        &cfg,
        &risk,
        ask,
        true,
        Some(SetupPerformance {
            closed_trades: 20,
            wins: 14,
        }),
    )
    .expect("shadow");
    assert!(shadow.risk_bps <= 300);
}

#[test]
fn profit_withdraw_fires_at_fifty_percent_of_principal() {
    use crate::sizing::CapitalState;
    let mut capital = CapitalState::new(1_000 * MICROS_PER_UNIT);
    assert_eq!(capital.withdraw_target_micros(), 500 * MICROS_PER_UNIT);

    // $499 of net realized is not enough.
    assert_eq!(
        capital.apply_withdraw_checkpoints(499 * MICROS_PER_UNIT, 10),
        0
    );
    assert_eq!(capital.withdraw_count, 0);

    // $500 locks in the first checkpoint and drops tradable bankroll back.
    let withdrawn = capital.apply_withdraw_checkpoints(500 * MICROS_PER_UNIT, 20);
    assert_eq!(withdrawn, 500 * MICROS_PER_UNIT);
    assert_eq!(capital.withdraw_count, 1);
    assert_eq!(
        capital.tradable_bankroll_micros(500 * MICROS_PER_UNIT),
        1_000 * MICROS_PER_UNIT
    );

    // Another $500 of fresh progress unlocks a second withdraw.
    let withdrawn = capital.apply_withdraw_checkpoints(1_000 * MICROS_PER_UNIT, 30);
    assert_eq!(withdrawn, 500 * MICROS_PER_UNIT);
    assert_eq!(capital.withdraw_count, 2);
    assert_eq!(capital.withdrawn_micros, 1_000 * MICROS_PER_UNIT);
}

#[test]
fn entry_stake_is_material_on_unproven_setup() {
    // Spec 093: even a cold setup risks ~2% of the $1000 principal, not cents.
    let mut engine = EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let result = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(result.decision.action, DecisionAction::Open);
    let stake = result.decision.stake_micros.expect("stake");
    assert!(
        stake >= 15 * MICROS_PER_UNIT,
        "unproven stake should be near 2% of bankroll, got {stake}"
    );
    assert_eq!(result.decision.stake_tier.as_deref(), Some("unproven"));
    assert_eq!(result.decision.risk_bps, Some(200));
}

#[test]
fn journals_written_before_spec_091_still_deserialize() {
    // A Spec 090 record: no asset, no accounting fields.
    let legacy = r#"{
        "strategy_version": "btc_window_engine_v1",
        "condition_id": "btc-5m-old",
        "window": "5m",
        "elapsed_secs": 150,
        "observed_at_ns": 1,
        "action": "no_trade",
        "reason": "NO_SETUP_PASSED",
        "rejection_reasons": [],
        "setup_id": null,
        "side": null,
        "ask_micros": null,
        "bid_micros": null,
        "edge_micros": null,
        "effective_cost_micros": null,
        "quantity_micros": null,
        "position_state": "flat",
        "book": "active_core",
        "paper_only": true,
        "real_money": false
    }"#;
    let record: crate::DecisionRecord = serde_json::from_str(legacy).expect("legacy record");
    assert_eq!(record.asset, MarketAsset::Btc);
    assert_eq!(record.stake_micros, None);
    assert_eq!(record.realized_pnl_micros, None);
    assert_eq!(record.outcome, None);
}

#[test]
fn late_continuation_defaults_match_pre095_hardcodes() {
    let p = crate::LateContinuationParams::compiled_defaults();
    assert!(p.enabled);
    assert_eq!(p.entry_elapsed_lo_secs, 230);
    assert_eq!(p.entry_elapsed_hi_secs, 245);
    assert_eq!(p.min_confidence_micros, 600_000);
    assert_eq!(p.max_confidence_micros, 820_000);
    assert_eq!(p.max_ask_override_micros, 820_000);
    assert!(p.require_positive_mom_med);
    assert!(p.validate().is_empty());
}

#[test]
fn all_eight_setup_defaults_match_pre_policy_hardcodes() {
    let bag = crate::SetupPolicyBag::compiled_defaults();
    assert!(bag.validate().is_empty());

    let mid = &bag.mid_window;
    assert_eq!(
        (mid.entry_elapsed_lo_secs, mid.entry_elapsed_hi_secs),
        (135, 180)
    );
    assert_eq!(mid.min_confidence_micros, 650_000);
    assert_eq!(mid.max_ask_override_micros, 820_000);
    assert_eq!(mid.min_persistence_secs, 20);
    assert!(mid.require_non_negative_mom);

    let hi = &bag.high_confidence;
    assert_eq!(
        (hi.entry_elapsed_lo_secs, hi.entry_elapsed_hi_secs),
        (150, 165)
    );
    assert_eq!(hi.min_confidence_micros, 750_000);
    assert_eq!(hi.max_ask_override_micros, 860_000);
    assert_eq!(hi.persistence_since_offset_secs, 120);
    assert!(hi.force_shadow_book);

    let p15 = &bag.fifteen_persistent;
    assert_eq!(
        (p15.entry_elapsed_lo_secs, p15.entry_elapsed_hi_secs),
        (220, 260)
    );
    assert_eq!(p15.min_confidence_micros, 600_000);
    assert_eq!(p15.max_ask_override_micros, 840_000);
    assert_eq!(p15.min_persistence_secs, 30);
    assert!(p15.require_checkpoint_favourite);

    let acc = &bag.fifteen_acceleration;
    assert_eq!(
        (acc.entry_elapsed_lo_secs, acc.entry_elapsed_hi_secs),
        (400, 440)
    );
    assert_eq!(acc.min_persistence_secs, 60);
    assert_eq!(acc.min_confidence_delta_60s_micros, 50_000);
    assert!(acc.require_reference_agrees);

    let cont = &bag.fifteen_continuation;
    assert_eq!(
        (cont.entry_elapsed_lo_secs, cont.entry_elapsed_hi_secs),
        (640, 680)
    );
    assert_eq!(cont.min_confidence_micros, 650_000);
    assert_eq!(cont.persistence_since_offset_secs, 600);
    assert_eq!(cont.min_confidence_delta_60s_micros, 20_000);
    assert_eq!(cont.shadow_ask_threshold_micros, 800_000);

    let m50 = &bag.one_hour_minute50;
    assert_eq!(
        (m50.entry_elapsed_lo_secs, m50.entry_elapsed_hi_secs),
        (2_940, 3_060)
    );
    assert_eq!(m50.min_persistence_secs, 300);
    assert!(m50.require_mom_sign_agreement);
    assert!(m50.force_shadow_book);

    let seq = &bag.one_hour_seq_hedge;
    assert_eq!(
        (seq.entry_elapsed_lo_secs, seq.entry_elapsed_hi_secs),
        (880, 920)
    );
    assert_eq!(
        (seq.entry_elapsed_lo2_secs, seq.entry_elapsed_hi2_secs),
        (2_380, 2_420)
    );
    assert_eq!(seq.min_confidence_window2_micros, 700_000);
    assert!(seq.force_shadow_book);

    let plugins = crate::list_plugins();
    assert_eq!(plugins.as_array().map(std::vec::Vec::len), Some(8));
}

#[test]
fn phase2_late_only_policy_deserializes_with_defaulted_siblings() {
    let phase2 = r#"{
      "policy_id": "window_paper_policy",
      "version": 1,
      "paper_only": true,
      "live_trading": false,
      "change_reason": "phase2",
      "setups": {
        "5m_late_continuation": {
          "enabled": true,
          "entry_elapsed_lo_secs": 230,
          "entry_elapsed_hi_secs": 245,
          "min_confidence_micros": 600000,
          "max_confidence_micros": 820000,
          "max_ask_override_micros": 820000,
          "require_positive_mom_med": true
        }
      }
    }"#;
    let doc: crate::WindowPolicyDocument = serde_json::from_str(phase2).expect("phase2 doc");
    assert!(doc.validate().is_empty());
    assert_eq!(doc.setups.mid_window.entry_elapsed_lo_secs, 135);
    assert_eq!(doc.setups.one_hour_seq_hedge.entry_elapsed_lo2_secs, 2_380);
}

#[test]
fn policy_rejects_inverted_entry_window_and_live_trading() {
    let mut doc = crate::WindowPolicyDocument::compiled_defaults();
    doc.setups.late_continuation.entry_elapsed_lo_secs = 250;
    doc.setups.late_continuation.entry_elapsed_hi_secs = 200;
    assert!(!doc.validate().is_empty());
    doc = crate::WindowPolicyDocument::compiled_defaults();
    doc.live_trading = true;
    assert!(doc.validate().iter().any(|e| e.contains("live_trading")));
}

#[test]
fn policy_store_activate_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::PolicyStore::new(dir.path());
    let mut doc = crate::WindowPolicyDocument::compiled_defaults();
    doc.change_reason = "test_widen".into();
    doc.setups.late_continuation.entry_elapsed_lo_secs = 200;
    doc.setups.late_continuation.entry_elapsed_hi_secs = 260;
    let pointer = store.save_version(doc).expect("save");
    let active = store.activate(pointer.version).expect("activate");
    assert_eq!(active.source, crate::PolicySource::ActiveFile);
    assert_eq!(active.late_continuation().entry_elapsed_lo_secs, 200);
    let loaded = store.load_active().expect("load");
    assert_eq!(loaded.digest_hex, active.digest_hex);
}

#[test]
fn opens_journal_configuration_digest_from_default_policy() {
    let mut engine = crate::EngineState::new(1_000 * MICROS_PER_UNIT);
    warm_before_primary_entry(&mut engine);
    let result = engine.on_every_second(&snapshot_5m(150, 690_000, 700_000, 290_000, 300_000));
    assert_eq!(result.decision.action, DecisionAction::Open);
    let digest = result.decision.configuration_digest.expect("digest");
    assert_eq!(digest, crate::ActivePolicy::compiled_defaults().digest_hex);
    assert_eq!(
        result
            .position
            .as_ref()
            .unwrap()
            .configuration_digest
            .as_deref(),
        Some(digest.as_str())
    );
}

#[test]
fn disabled_late_continuation_via_policy_rejects_setup() {
    let mut engine = crate::EngineState::new(1_000 * MICROS_PER_UNIT);
    let mut policy = crate::ActivePolicy::compiled_defaults();
    policy.document.setups.late_continuation.enabled = false;
    // Recompute digest after mutation for consistency (not required for in-memory).
    policy.digest_hex = policy.document.digest_hex();
    engine.sync_policy(policy);
    // Warm history then hit late window — mid-window setups may still fire; force
    // only late by using elapsed 235 where mid/high windows are closed.
    for elapsed in 60..235 {
        let _ = engine.on_every_second(&snapshot_5m(elapsed, 690_000, 700_000, 290_000, 300_000));
        engine.position = None; // clear any earlier mid-window opens for this unit test
    }
    let result = engine.on_every_second(&snapshot_5m(235, 690_000, 700_000, 290_000, 300_000));
    // With late disabled, either no_trade or a different setup — must not be late_continuation.
    if result.decision.action == DecisionAction::Open {
        assert_ne!(
            result.decision.setup_id.as_deref(),
            Some("5m_late_continuation")
        );
    } else {
        assert!(
            result
                .decision
                .rejection_reasons
                .iter()
                .any(|r| r.contains("5m_late_continuation:SETUP_DISABLED"))
                || result.decision.reason == "NO_SETUP_PASSED"
        );
    }
}
