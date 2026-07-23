#![forbid(unsafe_code)]

use integration_daemon::{run_soak, FaultScript, SoakPlan};
use market_recorder::{SegmentConfig, SegmentedJournalWriter};
use session_runtime::{
    read_checkpoint, recover_segmented, write_checkpoint, DurableCoordinator, PrefixCheckpoint,
    RecoveryState,
};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Soak {
        journal: PathBuf,
        checkpoint: PathBuf,
        start_time_ms: i64,
        hours: u16,
        ticks_per_hour: u16,
    },
    Recover {
        journal: PathBuf,
        checkpoint: Option<PathBuf>,
    },
}

#[derive(Debug, Eq, PartialEq)]
struct UsageError;

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "usage: integration-daemon soak <journal-dir> <checkpoint> <start-ms> <hours> <ticks-per-hour> | integration-daemon recover <journal-dir> [checkpoint]",
        )
    }
}

impl Error for UsageError {}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("read-only integration daemon failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    match parse_command(env::args_os().skip(1))? {
        Command::Soak {
            journal,
            checkpoint,
            start_time_ms,
            hours,
            ticks_per_hour,
        } => {
            let writer = SegmentedJournalWriter::open(&journal, SegmentConfig::default())?;
            if writer.last_sequence().is_some() {
                return Err("soak requires an empty session journal".into());
            }
            let durable = DurableCoordinator::new(
                writer,
                RecoveryState {
                    coordinator: market_session::MarketSessionCoordinator::default(),
                    last_sequence: None,
                },
            )?;
            let (report, mut durable) = run_soak(
                durable,
                SoakPlan {
                    start_time_ms,
                    hours,
                    ticks_per_hour,
                },
                FaultScript::default(),
            )?;
            durable.sync()?;
            let sequence = report.last_sequence.ok_or("soak journal is empty")?;
            write_checkpoint(
                checkpoint,
                PrefixCheckpoint {
                    sequence,
                    coordinator_digest: report.coordinator_digest,
                },
            )?;
            println!(
                "ticks={} sessions={} finalized={} ready={} degraded={} sequence={} digest={}",
                report.ticks,
                report.generated_sessions,
                report.finalized_sessions,
                report.ready_session_observations,
                report.degraded_session_observations,
                sequence,
                hex(&report.coordinator_digest),
            );
        }
        Command::Recover {
            journal,
            checkpoint,
        } => {
            let checkpoint = checkpoint.map(read_checkpoint).transpose()?;
            let recovered = recover_segmented(journal, checkpoint)?;
            println!(
                "sequence={:?} sessions={} halted={} digest={}",
                recovered.last_sequence,
                recovered.coordinator.snapshot().sessions.len(),
                recovered.coordinator.snapshot().halted,
                hex(&recovered.coordinator.snapshot().digest),
            );
        }
    }
    Ok(())
}

fn parse_command(mut arguments: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    match arguments.next().and_then(|value| value.into_string().ok()) {
        Some(command) if command == "soak" => {
            let journal = PathBuf::from(arguments.next().ok_or(UsageError)?);
            let checkpoint = PathBuf::from(arguments.next().ok_or(UsageError)?);
            let start_time_ms = parse(&arguments.next().ok_or(UsageError)?)?;
            let hours = parse(&arguments.next().ok_or(UsageError)?)?;
            let ticks_per_hour = parse(&arguments.next().ok_or(UsageError)?)?;
            if arguments.next().is_some() {
                return Err(UsageError);
            }
            Ok(Command::Soak {
                journal,
                checkpoint,
                start_time_ms,
                hours,
                ticks_per_hour,
            })
        }
        Some(command) if command == "recover" => {
            let journal = PathBuf::from(arguments.next().ok_or(UsageError)?);
            let checkpoint = arguments.next().map(PathBuf::from);
            if arguments.next().is_some() {
                return Err(UsageError);
            }
            Ok(Command::Recover {
                journal,
                checkpoint,
            })
        }
        _ => Err(UsageError),
    }
}

fn parse<T: std::str::FromStr>(value: &OsString) -> Result<T, UsageError> {
    value
        .to_str()
        .ok_or(UsageError)?
        .parse()
        .map_err(|_| UsageError)
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
    fn parses_bounded_command_shapes() {
        assert_eq!(
            parse_command(
                ["soak", "journal", "checkpoint", "3600000", "3", "4"]
                    .map(Into::into)
                    .into_iter()
            ),
            Ok(Command::Soak {
                journal: PathBuf::from("journal"),
                checkpoint: PathBuf::from("checkpoint"),
                start_time_ms: 3_600_000,
                hours: 3,
                ticks_per_hour: 4,
            })
        );
        assert_eq!(
            parse_command(["recover", "journal"].map(Into::into).into_iter()),
            Ok(Command::Recover {
                journal: PathBuf::from("journal"),
                checkpoint: None,
            })
        );
        assert!(parse_command(std::iter::empty()).is_err());
        assert!(parse_command(["soak"].map(Into::into).into_iter()).is_err());
        assert!(parse_command(["unknown"].map(Into::into).into_iter()).is_err());
    }

    #[test]
    fn digest_hex_is_stable() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }
}
