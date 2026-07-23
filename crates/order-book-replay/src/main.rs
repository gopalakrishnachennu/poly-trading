#![forbid(unsafe_code)]

use order_book_replay::{
    read_checkpoint, replay_path, replay_segmented_from_checkpoint, replay_segmented_path,
    write_checkpoint, ReplayState,
};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let command = match parse_command(env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match execute(command) {
        Ok(state) => {
            println!(
                "epoch={} status={:?} sequence={:?} books={} digest={}",
                state.epoch(),
                state.status(),
                state.last_sequence(),
                state.books().len(),
                hex(&state.digest()),
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("order-book replay failed: {error}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: order-book-replay <journal> | --segments <directory> | --segments-checkpoint <directory> <checkpoint> | --write-segment-checkpoint <directory> <new-checkpoint> | --read-checkpoint <checkpoint>";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Single(PathBuf),
    Segments(PathBuf),
    SegmentsCheckpoint {
        directory: PathBuf,
        checkpoint: PathBuf,
    },
    WriteSegmentCheckpoint {
        directory: PathBuf,
        checkpoint: PathBuf,
    },
    ReadCheckpoint(PathBuf),
}

fn parse_command(mut arguments: impl Iterator<Item = OsString>) -> Result<Command, &'static str> {
    let first = arguments.next().ok_or(USAGE)?;
    let command = match first.to_str() {
        Some("--segments") => Command::Segments(PathBuf::from(arguments.next().ok_or(USAGE)?)),
        Some("--segments-checkpoint") => Command::SegmentsCheckpoint {
            directory: PathBuf::from(arguments.next().ok_or(USAGE)?),
            checkpoint: PathBuf::from(arguments.next().ok_or(USAGE)?),
        },
        Some("--write-segment-checkpoint") => Command::WriteSegmentCheckpoint {
            directory: PathBuf::from(arguments.next().ok_or(USAGE)?),
            checkpoint: PathBuf::from(arguments.next().ok_or(USAGE)?),
        },
        Some("--read-checkpoint") => {
            Command::ReadCheckpoint(PathBuf::from(arguments.next().ok_or(USAGE)?))
        }
        Some(value) if value.starts_with('-') => return Err(USAGE),
        _ => Command::Single(PathBuf::from(first)),
    };
    if arguments.next().is_some() {
        return Err(USAGE);
    }
    Ok(command)
}

fn execute(command: Command) -> Result<ReplayState, Box<dyn Error>> {
    match command {
        Command::Single(path) => Ok(replay_path(path)?),
        Command::Segments(path) => Ok(replay_segmented_path(path)?),
        Command::SegmentsCheckpoint {
            directory,
            checkpoint,
        } => {
            let checkpoint = read_checkpoint(checkpoint)?;
            Ok(replay_segmented_from_checkpoint(directory, &checkpoint)?)
        }
        Command::WriteSegmentCheckpoint {
            directory,
            checkpoint,
        } => {
            let state = replay_segmented_path(directory)?;
            write_checkpoint(checkpoint, &state)?;
            Ok(state)
        }
        Command::ReadCheckpoint(path) => Ok(read_checkpoint(path)?),
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
    fn digest_hex_is_stable() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn parses_bounded_commands() {
        assert_eq!(
            parse_command(["capture.journal"].map(Into::into).into_iter()),
            Ok(Command::Single(PathBuf::from("capture.journal")))
        );
        assert_eq!(
            parse_command(["--segments", "segments"].map(Into::into).into_iter()),
            Ok(Command::Segments(PathBuf::from("segments")))
        );
        assert!(parse_command(std::iter::empty()).is_err());
        assert!(parse_command(["--segments"].map(Into::into).into_iter()).is_err());
        assert!(parse_command(["one", "two"].map(Into::into).into_iter()).is_err());
    }
}
