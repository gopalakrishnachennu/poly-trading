#![forbid(unsafe_code)]

//! Read-only public capture routed through bounded authoritative live state.

use live_market_state::{spawn_actor, ActorConfig, ActorSnapshot};
use market_recorder::{EventJournal, JournalWriter, SegmentConfig, SegmentedJournalWriter};
use public_market_data::{
    capture_session_with_channel, CaptureConfig, CaptureError, CaptureOutcome, DiscoveryConfig,
    DiscoveryWindow, GammaDiscoveryClient, MarketIdentity,
};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

const LOOKBACK_MS: i64 = 90 * 60 * 1_000;
const LOOKAHEAD_MS: i64 = 3 * 60 * 60 * 1_000;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("live market state failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    match parse_target(env::args_os().skip(1))? {
        JournalTarget::Single(path) => run_with_journal(JournalWriter::open(path)?).await,
        JournalTarget::Segments(path) => {
            run_with_journal(SegmentedJournalWriter::open(
                path,
                SegmentConfig::default(),
            )?)
            .await
        }
    }
}

async fn run_with_journal<J: EventJournal>(mut journal: J) -> Result<(), Box<dyn Error>> {
    let mut sequence = match journal.last_event_sequence() {
        Some(last) => last.checked_add(1).ok_or("journal sequence exhausted")?,
        None => 0,
    };
    let discovery = GammaDiscoveryClient::new(DiscoveryConfig::default())?;
    let runtime = spawn_actor(ActorConfig::default())?;
    let sender = runtime.sender.clone();

    let capture_result = capture_forever(&discovery, &mut journal, &mut sequence, &sender).await;
    drop(sender);
    drop(runtime.sender);
    let terminal = runtime.task.await?;
    print_terminal(&terminal);
    capture_result
}

#[derive(Debug, Eq, PartialEq)]
enum JournalTarget {
    Single(PathBuf),
    Segments(PathBuf),
}

fn parse_target(
    mut arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<JournalTarget, &'static str> {
    const USAGE: &str = "usage: live-market-state <journal-path> | --segments <segment-directory>";
    let first = arguments.next().ok_or(USAGE)?;
    let target = if first == "--segments" {
        JournalTarget::Segments(PathBuf::from(arguments.next().ok_or(USAGE)?))
    } else if first.to_string_lossy().starts_with('-') {
        return Err(USAGE);
    } else {
        JournalTarget::Single(PathBuf::from(first))
    };
    if arguments.next().is_some() {
        return Err(USAGE);
    }
    Ok(target)
}

async fn capture_forever<J: EventJournal>(
    discovery: &GammaDiscoveryClient,
    journal: &mut J,
    sequence: &mut u64,
    sender: &mpsc::Sender<event_schema::EventEnvelope>,
) -> Result<(), Box<dyn Error>> {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let markets = match discover_current(discovery).await {
            Ok(markets) if !markets.is_empty() => markets,
            Ok(_) => {
                eprintln!("no eligible hourly markets; retrying in {backoff:?}");
                sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
            Err(error) => {
                eprintln!("market discovery failed: {error}; retrying in {backoff:?}");
                sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        eprintln!(
            "capturing {} markets through bounded live state",
            markets.len()
        );
        match capture_session_with_channel(
            &CaptureConfig::default(),
            &markets,
            journal,
            sequence,
            Some(sender),
        )
        .await
        {
            Ok(CaptureOutcome::Shutdown) => return Ok(()),
            Ok(CaptureOutcome::Rediscover) => backoff = INITIAL_BACKOFF,
            Err(error @ (CaptureError::LiveChannelFull | CaptureError::LiveChannelClosed)) => {
                return Err(Box::new(error));
            }
            Err(error) => {
                eprintln!("capture epoch ended: {error}; rediscovering in {backoff:?}");
                sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

async fn discover_current(
    discovery: &GammaDiscoveryClient,
) -> Result<Vec<MarketIdentity>, Box<dyn Error>> {
    let now = now_ms()?;
    let start = now
        .checked_sub(LOOKBACK_MS)
        .ok_or("discovery window underflow")?;
    let end = now
        .checked_add(LOOKAHEAD_MS)
        .ok_or("discovery window overflow")?;
    Ok(discovery
        .discover(DiscoveryWindow::new(start, end)?)
        .await?)
}

fn now_ms() -> Result<i64, Box<dyn Error>> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(i64::try_from(duration.as_millis())?)
}

fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_BACKOFF)
}

fn print_terminal(snapshot: &ActorSnapshot) {
    println!(
        "mode={:?} ready={} epoch={} sequence={:?} books={} digest={}",
        snapshot.mode,
        snapshot.ready,
        snapshot.epoch,
        snapshot.last_sequence,
        snapshot.book_count,
        hex(&snapshot.digest),
    );
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_backoff_are_bounded() {
        assert!(parse_target(std::iter::empty()).is_err());
        assert!(parse_target(["one", "two"].map(Into::into).into_iter()).is_err());
        assert_eq!(
            parse_target(["capture.journal"].map(Into::into).into_iter()),
            Ok(JournalTarget::Single(PathBuf::from("capture.journal")))
        );
        assert_eq!(
            parse_target(["--segments", "segments"].map(Into::into).into_iter()),
            Ok(JournalTarget::Segments(PathBuf::from("segments")))
        );
        assert_eq!(next_backoff(Duration::from_secs(20)), MAX_BACKOFF);
    }

    #[test]
    fn digest_hex_is_stable() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }
}
