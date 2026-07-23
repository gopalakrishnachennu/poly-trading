#![forbid(unsafe_code)]

use shadow_ops::{run_stress_profile, StressProfile};
use std::env;
use std::error::Error;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("shadow operations failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    if arguments.next().as_deref() != Some("stress") {
        return Err("usage: shadow-ops stress <smoke|day|seven-day>".into());
    }
    let profile = match arguments.next().as_deref() {
        Some("smoke") => StressProfile::Smoke,
        Some("day") => StressProfile::OneDay,
        Some("seven-day") => StressProfile::SevenDay,
        _ => return Err("unknown stress profile".into()),
    };
    if arguments.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let report = run_stress_profile(profile)?;
    println!(
        "profile={:?} ticks={} sessions={} finalized={} ready={} records={} bytes={} syncs={} digest={}",
        report.profile,
        report.ticks,
        report.sessions,
        report.finalized_sessions,
        report.ready_observations,
        report.records,
        report.encoded_bytes,
        report.syncs,
        hex(&report.coordinator_digest),
    );
    Ok(())
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
}
