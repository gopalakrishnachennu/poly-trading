//! Managed multi-asset window paper worker.
//!
//! Polls the unified `/api/windows` observation feed, evaluates
//! `btc_window_engine_v1` across every configured window asset, and journals
//! paper-only decisions with their stake and result. Grants no live trading,
//! Paper Week capital, signing, or submission authority.

use btc_window_strategy::{PaperRuntime, RuntimeConfig, WindowsFeed, MICROS_PER_UNIT};
use std::env;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

async fn fetch_windows(client: &reqwest::Client, url: &str) -> Result<WindowsFeed, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let body = response.text().await.map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

fn write_status_file(path: &PathBuf, payload: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, payload).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn write_boot_status(path: &PathBuf, observed_at_ns: i64) {
    let payload = serde_json::json!({
        "paper_only": true,
        "real_money": false,
        "strategy_version": btc_window_strategy::STRATEGY_VERSION,
        "live_trading_authority": false,
        "paper_week_capital_authority": false,
        "observed_at_ns": observed_at_ns,
        "markets_seen": 0,
        "decisions_this_tick": 0,
        "open_positions": 0,
        "assets_evaluated": [],
        "settlements_this_tick": 0,
        "ledger": btc_window_strategy::LedgerSnapshot::default(),
        "recovery": btc_window_strategy::RecoverySummary::default(),
        "settled_on_recovery": 0,
        "journal_errors": [],
        "capital": btc_window_strategy::CapitalState::new(1_000 * MICROS_PER_UNIT),
        "tradable_bankroll_micros": 1_000 * MICROS_PER_UNIT,
        "net_realized_micros": 0,
        "withdraw_target_micros": 500 * MICROS_PER_UNIT,
        "withdraw_progress_bps": 0,
        "last_actions": [],
        "reject_reasons": ["BOOT"],
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&payload) {
        write_status_file(path, &bytes);
    }
}

#[tokio::main]
async fn main() {
    let windows_url =
        env::var("POLY_WINDOWS_URL").unwrap_or_else(|_| "http://127.0.0.1:3000/api/windows".into());
    let interval_ms: u64 = env::var("POLY_BTC_WINDOW_PAPER_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000);
    let bankroll = env::var("POLY_BTC_WINDOW_PAPER_BANKROLL_MICROS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000 * MICROS_PER_UNIT);
    let journal_dir = PathBuf::from(
        env::var("POLY_BTC_WINDOW_PAPER_DIR").unwrap_or_else(|_| "var/btc-window-paper".into()),
    );
    let policy_dir = PathBuf::from(
        env::var("POLY_STRATEGY_CONTROL_DIR").unwrap_or_else(|_| "var/strategy-control".into()),
    );

    let config = RuntimeConfig {
        bankroll_micros: bankroll,
        journal_dir: journal_dir.clone(),
        status_path: journal_dir.join("status.json"),
        policy_dir,
        ..RuntimeConfig::default()
    };
    let status_path = config.status_path.clone();
    // Two workers on one journal destroy the book: both append decisions and the
    // position checkpoint becomes last-writer-wins, so one worker's open position
    // is erased by the other's empty checkpoint (ADR 0096).
    let _book_lock = match btc_window_strategy::BookLock::acquire(&config.journal_dir) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("btc-window-paper: HALTED — {err}");
            std::process::exit(1);
        }
    };
    // Recovery is all-or-nothing (ADR 0096): refuse to start rather than trade
    // against a book that is quietly missing trades or open positions.
    let mut runtime = match PaperRuntime::recover(config, now_ns()) {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("btc-window-paper: HALTED — recovery failed: {err}");
            std::process::exit(1);
        }
    };
    let recovered = runtime.status.recovery.clone();
    eprintln!(
        "btc-window-paper: recovered closes={} skipped_pre_accounting={} positions={} from {}",
        recovered.closes_replayed,
        recovered.pre_accounting_closes_skipped,
        recovered.positions_restored,
        recovered.journal_path
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .expect("reqwest client");

    write_boot_status(&status_path, now_ns());
    eprintln!(
        "btc-window-paper: strategy={} paper_only=true real_money=false feed={windows_url} interval_ms={interval_ms}",
        btc_window_strategy::STRATEGY_VERSION
    );

    loop {
        match fetch_windows(&client, &windows_url).await {
            Ok(feed) => {
                let decisions = runtime.on_feed(&feed, now_ns());
                let opens = decisions
                    .iter()
                    .filter(|d| matches!(d.action, btc_window_strategy::DecisionAction::Open))
                    .count();
                let core = runtime.status.ledger.active_core;
                eprintln!(
                    "btc-window-paper: assets={} markets={} decisions={} opens={opens} positions={} settled={} core_realized={} bankroll={} withdrawn={} withdraw_progress_bps={}",
                    runtime.status.assets_evaluated.join("/"),
                    runtime.status.markets_seen,
                    runtime.status.decisions_this_tick,
                    runtime.status.open_positions,
                    runtime.status.settlements_this_tick,
                    core.realized_pnl_micros,
                    runtime.status.tradable_bankroll_micros,
                    runtime.status.capital.withdrawn_micros,
                    runtime.status.withdraw_progress_bps
                );
                if runtime.must_halt() {
                    eprintln!(
                        "btc-window-paper: HALTED — journal not durable: {}",
                        runtime.status.journal_errors.join("; ")
                    );
                    if let Ok(payload) = serde_json::to_vec_pretty(&runtime.status) {
                        write_status_file(&status_path, &payload);
                    }
                    std::process::exit(1);
                }
            }
            Err(err) => {
                eprintln!("btc-window-paper: feed unavailable: {err}");
                runtime.status.observed_at_ns = now_ns();
                runtime.status.reject_reasons = vec![format!("FEED_UNAVAILABLE:{err}")];
                if let Ok(payload) = serde_json::to_vec_pretty(&runtime.status) {
                    write_status_file(&status_path, &payload);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}
