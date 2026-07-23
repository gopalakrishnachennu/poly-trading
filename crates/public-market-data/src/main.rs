#![forbid(unsafe_code)]

//! Read-only hourly BTC/ETH public market recorder.

use market_recorder::JournalWriter;
use public_market_data::{
    capture_session, CaptureConfig, CaptureOutcome, DiscoveryConfig, DiscoveryWindow,
    GammaDiscoveryClient, MarketIdentity,
};
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

const LOOKBACK_MS: i64 = 90 * 60 * 1_000;
const LOOKAHEAD_MS: i64 = 3 * 60 * 60 * 1_000;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum Command {
    DiscoverOnly,
    Record { journal_path: PathBuf },
}

#[derive(Debug)]
struct UsageError;

impl Display for UsageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "usage: public-market-data --discover-only | public-market-data <journal-path>",
        )
    }
}

impl Error for UsageError {}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("public market recorder failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let command = parse_command(env::args_os().skip(1))?;
    let discovery = GammaDiscoveryClient::new(DiscoveryConfig::default())?;

    match command {
        Command::DiscoverOnly => {
            let markets = discover_current(&discovery).await?;
            print_markets(&markets);
            Ok(())
        }
        Command::Record { journal_path } => {
            let mut journal = JournalWriter::open(journal_path)?;
            record_forever(&discovery, &mut journal).await
        }
    }
}

fn parse_command(
    mut arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<Command, UsageError> {
    let first = arguments.next().ok_or(UsageError)?;
    if arguments.next().is_some() {
        return Err(UsageError);
    }
    if first == "--discover-only" {
        Ok(Command::DiscoverOnly)
    } else if first.to_string_lossy().starts_with('-') {
        Err(UsageError)
    } else {
        Ok(Command::Record {
            journal_path: PathBuf::from(first),
        })
    }
}

async fn record_forever(
    discovery: &GammaDiscoveryClient,
    journal: &mut JournalWriter,
) -> Result<(), Box<dyn Error>> {
    let mut sequence = match journal.last_sequence() {
        Some(last) => last.checked_add(1).ok_or("journal sequence exhausted")?,
        None => 0,
    };
    let mut backoff = INITIAL_BACKOFF;

    loop {
        let markets = match discover_current(discovery).await {
            Ok(markets) if !markets.is_empty() => markets,
            Ok(_) => {
                eprintln!("no eligible hourly markets discovered; retrying in {backoff:?}");
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

        eprintln!("subscribing to {} validated hourly markets", markets.len());
        match capture_session(&CaptureConfig::default(), &markets, journal, &mut sequence).await {
            Ok(CaptureOutcome::Shutdown) => return Ok(()),
            Ok(CaptureOutcome::Rediscover) => backoff = INITIAL_BACKOFF,
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
    let window = DiscoveryWindow::new(start, end)?;
    Ok(discovery.discover(window).await?)
}

fn now_ms() -> Result<i64, Box<dyn Error>> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(i64::try_from(duration.as_millis())?)
}

fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_BACKOFF)
}

fn print_markets(markets: &[MarketIdentity]) {
    for market in markets {
        println!(
            "{} {} {} {} {} {}",
            market.asset.as_str(),
            market.start_time_ms,
            market.condition_id,
            market.up_token_id,
            market.down_token_id,
            hex(&market.rules_fingerprint),
        );
    }
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
    fn parses_only_supported_commands() {
        assert!(matches!(
            parse_command(["--discover-only".into()].into_iter()),
            Ok(Command::DiscoverOnly)
        ));
        assert!(matches!(
            parse_command(["capture.journal".into()].into_iter()),
            Ok(Command::Record { .. })
        ));
        assert!(parse_command(std::iter::empty()).is_err());
        assert!(parse_command(["--unknown".into()].into_iter()).is_err());
        assert!(parse_command(["one".into(), "two".into()].into_iter()).is_err());
    }

    #[test]
    fn backoff_is_bounded() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(next_backoff(Duration::from_secs(20)), MAX_BACKOFF);
        assert_eq!(next_backoff(MAX_BACKOFF), MAX_BACKOFF);
    }

    #[test]
    fn hex_encoding_is_stable() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
