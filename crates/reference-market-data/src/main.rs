#![forbid(unsafe_code)]

use market_recorder::JournalWriter;
use reference_market_data::{capture_session, replay_path, CaptureConfig, CaptureOutcome};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio::time::sleep;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("reference market data failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let first = args
        .next()
        .ok_or("usage: reference-market-data <journal> | --replay <journal>")?;
    if first == "--replay" {
        let path = PathBuf::from(args.next().ok_or("missing journal path")?);
        if args.next().is_some() {
            return Err("unexpected arguments".into());
        }
        let state = replay_path(path)?;
        println!(
            "epoch={} health={:?} sequence={:?} digest={}",
            state.epoch(),
            state.health(),
            state.last_sequence(),
            hex(&state.digest())
        );
        return Ok(());
    }
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let mut journal = JournalWriter::open(PathBuf::from(first))?;
    let mut sequence = journal.last_sequence().map_or(Ok(0), |last| {
        last.checked_add(1).ok_or("journal sequence exhausted")
    })?;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match capture_session(&CaptureConfig::default(), &mut journal, &mut sequence).await {
            Ok(CaptureOutcome::Shutdown) => return Ok(()),
            Ok(CaptureOutcome::Rotate) => backoff = INITIAL_BACKOFF,
            Ok(CaptureOutcome::Disconnected) => {
                sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
            Err(error) => {
                eprintln!("reference capture epoch failed: {error}; retrying in {backoff:?}");
                sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_BACKOFF)
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
    fn backoff_is_bounded() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(next_backoff(MAX_BACKOFF), MAX_BACKOFF);
    }
    #[test]
    fn digest_hex_is_stable() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }
}
