use super::*;
use proptest::prelude::*;

fn identity(asset: Asset, start: i64) -> MarketIdentity {
    let name = asset.as_str().to_ascii_lowercase();
    MarketIdentity {
        asset,
        event_id: format!("event-{name}"),
        market_id: format!("market-{name}"),
        condition_id: format!("condition-{name}"),
        question_id: format!("question-{name}"),
        event_slug: format!("{name}-hourly"),
        market_slug: format!("{name}-market"),
        series_id: format!("series-{name}"),
        series_slug: format!("{name}-up-or-down-hourly"),
        title: format!("{} Up or Down", asset.as_str()),
        start_time_ms: start,
        end_time_ms: start + 3_600_000,
        resolution_source: "https://www.binance.com".to_owned(),
        description: "validated test rules".to_owned(),
        up_token_id: format!("{name}-up"),
        down_token_id: format!("{name}-down"),
        rules_fingerprint: [asset as u8 + 1; 32],
    }
}

fn raw_book(identity: &MarketIdentity, token: &str, now: i64) -> RawBook {
    RawBook {
        condition_id: identity.condition_id.clone(),
        token_id: token.to_owned(),
        timestamp: now.to_string(),
        hash: format!("hash-{token}"),
        tick_size: "0.001".to_owned(),
        bids: vec![
            RawLevel {
                price: "0.49".to_owned(),
                size: "10".to_owned(),
            },
            RawLevel {
                price: "0.48".to_owned(),
                size: "20".to_owned(),
            },
        ],
        asks: vec![
            RawLevel {
                price: "0.50".to_owned(),
                size: "8".to_owned(),
            },
            RawLevel {
                price: "0.51".to_owned(),
                size: "25".to_owned(),
            },
        ],
    }
}

fn reference(identity: &MarketIdentity, now: i64) -> RawReference {
    RawReference {
        symbol: match identity.asset {
            Asset::Bitcoin => "BTCUSDT",
            Asset::Ethereum => "ETHUSDT",
        }
        .to_owned(),
        price: match identity.asset {
            Asset::Bitcoin => "65952.123456",
            Asset::Ethereum => "3641.123456",
        }
        .to_owned(),
        candle_open_time_ms: identity.start_time_ms,
        candle_close_time_ms: identity.end_time_ms - 1,
        candle_open: match identity.asset {
            Asset::Bitcoin => "65974.900000",
            Asset::Ethereum => "3633.160000",
        }
        .to_owned(),
        received_at_ms: now,
    }
}

fn asset(identity: &MarketIdentity, now: i64) -> AssetProjection {
    let policy = ProjectionPolicy::default();
    let up = normalize_book(
        raw_book(identity, &identity.up_token_id, now),
        &identity.condition_id,
        &identity.up_token_id,
        now,
        policy,
    )
    .unwrap();
    let mut down_raw = raw_book(identity, &identity.down_token_id, now);
    down_raw.asks[0].price = "0.49".to_owned();
    down_raw.bids[0].price = "0.47".to_owned();
    down_raw.bids[1].price = "0.46".to_owned();
    let down = normalize_book(
        down_raw,
        &identity.condition_id,
        &identity.down_token_id,
        now,
        policy,
    )
    .unwrap();
    compose_asset(identity, up, down, &reference(identity, now), now, policy).unwrap()
}

#[test]
fn current_market_selection_requires_exact_btc_and_eth() {
    let start = 1_000_000;
    let btc = identity(Asset::Bitcoin, start);
    let eth = identity(Asset::Ethereum, start);
    let selected = select_current_markets(&[btc.clone(), eth.clone()], start + 1).unwrap();
    assert_eq!(selected[&Asset::Bitcoin], btc);
    assert_eq!(selected[&Asset::Ethereum], eth);
    assert_eq!(
        select_current_markets(std::slice::from_ref(&btc), start + 1).unwrap_err(),
        ProjectionError::MarketCardinality("ETH")
    );
    assert_eq!(
        select_current_markets(&[btc.clone(), btc, eth], start + 1).unwrap_err(),
        ProjectionError::MarketCardinality("BTC")
    );
}

#[test]
fn book_validation_rejects_substitution_staleness_crossing_and_bad_decimal() {
    let market = identity(Asset::Bitcoin, 1_000_000);
    let now = 1_100_000;
    let policy = ProjectionPolicy::default();
    let valid = normalize_book(
        raw_book(&market, &market.up_token_id, now),
        &market.condition_id,
        &market.up_token_id,
        now,
        policy,
    )
    .unwrap();
    assert_eq!(valid.best_bid_micros, "490000");
    assert_eq!(valid.best_ask_micros, "500000");

    let mut substituted = raw_book(&market, &market.up_token_id, now);
    substituted.token_id = "other".to_owned();
    assert_eq!(
        normalize_book(
            substituted,
            &market.condition_id,
            &market.up_token_id,
            now,
            policy
        )
        .unwrap_err(),
        ProjectionError::BookIdentity
    );

    let stale = raw_book(
        &market,
        &market.up_token_id,
        now - policy.maximum_book_age_ms - 1,
    );
    assert_eq!(
        normalize_book(
            stale,
            &market.condition_id,
            &market.up_token_id,
            now,
            policy
        )
        .unwrap_err(),
        ProjectionError::BookStale
    );

    let mut crossed = raw_book(&market, &market.up_token_id, now);
    crossed.bids[0].price = "0.50".to_owned();
    assert_eq!(
        normalize_book(
            crossed,
            &market.condition_id,
            &market.up_token_id,
            now,
            policy
        )
        .unwrap_err(),
        ProjectionError::BookShape
    );

    let mut malformed = raw_book(&market, &market.up_token_id, now);
    malformed.asks[0].price = "0.5000001".to_owned();
    assert_eq!(
        normalize_book(
            malformed,
            &market.condition_id,
            &market.up_token_id,
            now,
            policy
        )
        .unwrap_err(),
        ProjectionError::BookValue
    );
}

#[test]
fn deep_valid_book_is_projected_without_rejecting_the_market() {
    let market = identity(Asset::Bitcoin, 1_000_000);
    let now = 1_100_000;
    let mut raw = raw_book(&market, &market.up_token_id, now);
    raw.tick_size = "0.000001".to_owned();
    // The venue can legitimately return a book deeper than the earlier 5k
    // parser ceiling.  A valid deep book must remain available, while the
    // projection still publishes only the bounded top levels.
    raw.bids = (0..6_001)
        .map(|index| RawLevel {
            price: format!("0.{:06}", 490_000 - index),
            size: "1".to_owned(),
        })
        .collect();
    raw.asks = (0..6_001)
        .map(|index| RawLevel {
            price: format!("0.{:06}", 500_000 + index),
            size: "1".to_owned(),
        })
        .collect();

    let projected = normalize_book(
        raw,
        &market.condition_id,
        &market.up_token_id,
        now,
        ProjectionPolicy::default(),
    )
    .unwrap();
    assert_eq!(projected.bids.len(), PROJECTED_BOOK_LEVELS);
    assert_eq!(projected.asks.len(), PROJECTED_BOOK_LEVELS);
}

#[test]
fn rfc3339_book_timestamp_is_preserved() {
    let market = identity(Asset::Bitcoin, 1_000_000);
    let now = 1_704_067_200_000;
    let mut raw = raw_book(&market, &market.up_token_id, now);
    raw.timestamp = "2024-01-01T00:00:00Z".to_owned();
    let book = normalize_book(
        raw,
        &market.condition_id,
        &market.up_token_id,
        now,
        ProjectionPolicy::default(),
    )
    .unwrap();
    assert_eq!(book.source_timestamp_ms, now);
}

#[test]
fn composition_is_observation_only_and_rejects_skew_or_wrong_hour() {
    let market = identity(Asset::Bitcoin, 1_000_000);
    let now = market.start_time_ms + 10_000;
    let policy = ProjectionPolicy::default();
    let projection = asset(&market, now);
    assert_eq!(projection.pair.buy_pair_cost_micros, "990000");
    assert_eq!(projection.pair.raw_gap_micros, "10000");
    assert_eq!(projection.pair.observation, "raw_pair_below_one");
    assert_eq!(projection.pair.decision, "no_trade");

    let up = normalize_book(
        raw_book(&market, &market.up_token_id, now),
        &market.condition_id,
        &market.up_token_id,
        now,
        policy,
    )
    .unwrap();
    let down_time = now - policy.maximum_cross_book_skew_ms - 1;
    let down = normalize_book(
        raw_book(&market, &market.down_token_id, down_time),
        &market.condition_id,
        &market.down_token_id,
        now,
        policy,
    )
    .unwrap();
    assert_eq!(
        compose_asset(
            &market,
            up.clone(),
            down,
            &reference(&market, now),
            now,
            policy
        )
        .unwrap_err(),
        ProjectionError::BookSkew
    );

    let mut wrong = reference(&market, now);
    wrong.candle_open_time_ms += 3_600_000;
    let down = normalize_book(
        raw_book(&market, &market.down_token_id, now),
        &market.condition_id,
        &market.down_token_id,
        now,
        policy,
    )
    .unwrap();
    assert_eq!(
        compose_asset(&market, up, down, &wrong, now, policy).unwrap_err(),
        ProjectionError::ReferenceIdentity
    );
}

#[test]
fn partial_failure_and_staleness_clear_every_asset() {
    let start = 10_000;
    let now = start + 1_000;
    let btc = identity(Asset::Bitcoin, start);
    let eth = identity(Asset::Ethereum, start);
    let policy = ProjectionPolicy::default();
    let mut state = ProjectionState::new(policy, start).unwrap();
    state
        .publish_ready(vec![asset(&btc, now), asset(&eth, now)], now)
        .unwrap();
    assert_eq!(state.snapshot().mode, ProjectionMode::Ready);
    assert_eq!(state.snapshot().assets.len(), 2);
    assert!(state.snapshot().verify_digest());

    state
        .publish_unavailable(now + 1, "one complementary leg unavailable")
        .unwrap();
    assert_eq!(state.snapshot().mode, ProjectionMode::Stale);
    assert!(state.snapshot().assets.is_empty());
    assert!(state.snapshot().no_trade);

    let ready_at = now + 2;
    state
        .publish_ready(vec![asset(&btc, ready_at), asset(&eth, ready_at)], ready_at)
        .unwrap();
    state
        .evaluate_freshness(ready_at + policy.maximum_projection_age_ms + 1)
        .unwrap();
    assert_eq!(state.snapshot().mode, ProjectionMode::Stale);
    assert!(state.snapshot().assets.is_empty());
}

#[test]
fn atomic_asset_set_is_order_independent_but_rejects_duplicates() {
    let start = 10_000;
    let now = start + 1_000;
    let btc = identity(Asset::Bitcoin, start);
    let eth = identity(Asset::Ethereum, start);
    let mut state = ProjectionState::new(ProjectionPolicy::default(), start).unwrap();

    state
        .publish_ready(vec![asset(&eth, now), asset(&btc, now)], now)
        .unwrap();
    assert_eq!(state.snapshot().assets[0].asset, "BTC");
    assert_eq!(state.snapshot().assets[1].asset, "ETH");

    let next = now + 1;
    assert_eq!(
        state
            .publish_ready(vec![asset(&btc, next), asset(&btc, next)], next)
            .unwrap_err(),
        ProjectionError::AssetSet
    );
}

#[test]
fn rollover_replaces_exact_identity_without_carrying_old_books() {
    let first_start = 20_000;
    let second_start = first_start + 3_600_000;
    let policy = ProjectionPolicy::default();
    let mut state = ProjectionState::new(policy, first_start).unwrap();
    let first_now = first_start + 1;
    state
        .publish_ready(
            vec![
                asset(&identity(Asset::Bitcoin, first_start), first_now),
                asset(&identity(Asset::Ethereum, first_start), first_now),
            ],
            first_now,
        )
        .unwrap();
    state
        .publish_unavailable(second_start, "hourly rollover rediscovery")
        .unwrap();
    assert!(state.snapshot().assets.is_empty());
    let second_now = second_start + 1;
    state
        .publish_ready(
            vec![
                asset(&identity(Asset::Bitcoin, second_start), second_now),
                asset(&identity(Asset::Ethereum, second_start), second_now),
            ],
            second_now,
        )
        .unwrap();
    assert!(state
        .snapshot()
        .assets
        .iter()
        .all(|value| value.start_time_ms == second_start));
}

proptest! {
    #[test]
    fn excess_price_precision_never_enters_projection(extra in 1_u8..10) {
        let market = identity(Asset::Bitcoin, 1_000_000);
        let now = 1_001_000;
        let mut raw = raw_book(&market, &market.up_token_id, now);
        raw.asks[0].price = format!("0.500000{extra}");
        prop_assert_eq!(normalize_book(raw, &market.condition_id, &market.up_token_id, now, ProjectionPolicy::default()).unwrap_err(), ProjectionError::BookValue);
    }
}
