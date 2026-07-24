//! Faithful L2 research exporter.
//!
//! Streams a raw `POLYJNL1` capture journal through the audited decode path
//! (`JournalReader` -> `EventEnvelope` -> `decode_public_payload` ->
//! `ReplayState::apply`) and emits one JSONL row per accepted market event,
//! carrying the reconstructed top-N depth of every affected token book, trade
//! prints, and both venue event time and local receive time.
//!
//! Fidelity is provable, not hoped: the exporter finishes by printing the same
//! `epoch/status/sequence/books/digest` line as the `order-book-replay` CLI.
//! Running both over the same journal must produce identical digests; any
//! divergence means the export is not faithful and must not be used.
//!
//! This is a read-only research tool. It holds no credential or order path and
//! its output is derived data; the raw journal remains authoritative.
//!
//! Usage:
//!   `export_l2 <journal> <output.jsonl> [--depth N]`
//!   `export_l2 --segments <directory> <output.jsonl> [--depth N]`

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use event_schema::{EventEnvelope, EventSource};
use market_recorder::{JournalReader, JournalTail, SegmentedJournalReader};
use order_book_replay::{BookKey, ReplayState};
use public_market_data::{decode_public_payload, MarketSide, PublicMarketEvent};
use serde_json::json;

const USAGE: &str =
    "usage: export_l2 <journal> <output.jsonl> [--depth N] | export_l2 --segments <directory> <output.jsonl> [--depth N]";
const DEFAULT_DEPTH: usize = 10;

struct Config {
    source: Source,
    output: PathBuf,
    depth: usize,
}

enum Source {
    Single(PathBuf),
    Segments(PathBuf),
}

enum Reader {
    Single(JournalReader),
    Segments(SegmentedJournalReader),
}

impl Reader {
    fn next_event(&mut self) -> Result<Option<EventEnvelope>, Box<dyn Error>> {
        match self {
            Self::Single(reader) => Ok(reader.next_event()?),
            Self::Segments(reader) => Ok(reader.next_event()?),
        }
    }

    fn clean_tail(&self) -> bool {
        match self {
            Self::Single(reader) => reader.tail() == Some(JournalTail::Clean),
            // The segmented reader validates every segment tail and the
            // cross-segment sequence internally; reaching end-of-stream without
            // error is its clean-completion signal.
            Self::Segments(_) => true,
        }
    }
}

fn parse_args(mut arguments: impl Iterator<Item = OsString>) -> Result<Config, &'static str> {
    let first = arguments.next().ok_or(USAGE)?;
    let (source, output) = if first.to_str() == Some("--segments") {
        let directory = arguments.next().ok_or(USAGE)?;
        let output = arguments.next().ok_or(USAGE)?;
        (
            Source::Segments(PathBuf::from(directory)),
            PathBuf::from(output),
        )
    } else {
        let output = arguments.next().ok_or(USAGE)?;
        (Source::Single(PathBuf::from(first)), PathBuf::from(output))
    };
    let mut depth = DEFAULT_DEPTH;
    match arguments.next() {
        None => {}
        Some(flag) if flag.to_str() == Some("--depth") => {
            depth = arguments
                .next()
                .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
                .filter(|parsed| (1..=100).contains(parsed))
                .ok_or(USAGE)?;
        }
        Some(_) => return Err(USAGE),
    }
    if arguments.next().is_some() {
        return Err(USAGE);
    }
    Ok(Config {
        source,
        output,
        depth,
    })
}

fn side_label(side: MarketSide) -> &'static str {
    match side {
        MarketSide::Bid => "bid",
        MarketSide::Ask => "ask",
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn book_row(
    state: &ReplayState,
    envelope: &EventEnvelope,
    epoch: u64,
    condition_id: &str,
    asset_id: &str,
    depth: usize,
) -> Option<serde_json::Value> {
    let key = BookKey {
        condition_id: condition_id.to_owned(),
        asset_id: asset_id.to_owned(),
    };
    let book = state.books().get(&key)?;
    let levels = |side: Vec<(common_types::PriceMicros, common_types::QuantityMicros)>| -> Vec<serde_json::Value> {
        side.into_iter()
            .map(|(price, size)| json!([price.as_micros(), size.as_micros()]))
            .collect()
    };
    Some(json!({
        "k": "book",
        "seq": envelope.sequence,
        "t": envelope.event_time_ns,
        "r": envelope.received_time_ns,
        "epoch": epoch,
        "cond": condition_id,
        "tok": asset_id,
        "auth": book.is_authoritative(),
        "bids": levels(book.top_bids(depth)),
        "asks": levels(book.top_asks(depth)),
    }))
}

#[allow(clippy::too_many_lines)]
fn run(config: &Config) -> Result<(), Box<dyn Error>> {
    let mut reader = match &config.source {
        Source::Single(path) => Reader::Single(JournalReader::open(path)?),
        Source::Segments(path) => Reader::Segments(SegmentedJournalReader::open(path)?),
    };
    let mut state = ReplayState::default();
    let mut output = BufWriter::new(File::create(&config.output)?);
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut last_epoch: Option<u64> = None;

    while let Some(envelope) = reader.next_event()? {
        // Integrity halts here exactly as in replay; a corrupt journal must
        // never silently continue into the research dataset.
        state.apply(&envelope)?;
        let epoch = state.epoch();

        if last_epoch != Some(epoch) {
            last_epoch = Some(epoch);
            let row = json!({
                "k": "epoch",
                "seq": envelope.sequence,
                "t": envelope.event_time_ns,
                "r": envelope.received_time_ns,
                "epoch": epoch,
            });
            serde_json::to_writer(&mut output, &row)?;
            output.write_all(b"\n")?;
            *counts.entry("epoch").or_default() += 1;
        }

        match envelope.source {
            EventSource::System => {
                // Identity and rules provenance arrive as JSON system payloads;
                // control markers (epoch start) are non-JSON and already
                // represented by the epoch rows above.
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&envelope.payload) {
                    let row = json!({
                        "k": "sys",
                        "seq": envelope.sequence,
                        "t": envelope.event_time_ns,
                        "r": envelope.received_time_ns,
                        "epoch": epoch,
                        "market": envelope.market_id,
                        "payload": value,
                    });
                    serde_json::to_writer(&mut output, &row)?;
                    output.write_all(b"\n")?;
                    *counts.entry("sys").or_default() += 1;
                }
            }
            EventSource::Market => {
                // `apply` accepted the envelope, so this decode cannot fail.
                let decoded = decode_public_payload(&envelope.payload)?;
                match decoded.event {
                    PublicMarketEvent::Book(snapshot) => {
                        if let Some(row) = book_row(
                            &state,
                            &envelope,
                            epoch,
                            &snapshot.condition_id,
                            &snapshot.asset_id,
                            config.depth,
                        ) {
                            serde_json::to_writer(&mut output, &row)?;
                            output.write_all(b"\n")?;
                            *counts.entry("book").or_default() += 1;
                        }
                    }
                    PublicMarketEvent::PriceChanges {
                        condition_id,
                        changes,
                    } => {
                        let mut tokens: Vec<&str> = changes
                            .iter()
                            .map(|change| change.asset_id.as_str())
                            .collect();
                        tokens.sort_unstable();
                        tokens.dedup();
                        for token in tokens {
                            if let Some(row) = book_row(
                                &state,
                                &envelope,
                                epoch,
                                &condition_id,
                                token,
                                config.depth,
                            ) {
                                serde_json::to_writer(&mut output, &row)?;
                                output.write_all(b"\n")?;
                                *counts.entry("book").or_default() += 1;
                            }
                        }
                    }
                    PublicMarketEvent::LastTrade(trade) => {
                        let row = json!({
                            "k": "trade",
                            "seq": envelope.sequence,
                            "t": envelope.event_time_ns,
                            "r": envelope.received_time_ns,
                            "epoch": epoch,
                            "cond": trade.condition_id,
                            "tok": trade.asset_id,
                            "side": side_label(trade.side),
                            "px": trade.price.as_micros(),
                            "qty": trade.quantity.map(common_types::QuantityMicros::as_micros),
                        });
                        serde_json::to_writer(&mut output, &row)?;
                        output.write_all(b"\n")?;
                        *counts.entry("trade").or_default() += 1;
                    }
                    PublicMarketEvent::BestBidAsk(update) => {
                        let row = json!({
                            "k": "bba",
                            "seq": envelope.sequence,
                            "t": envelope.event_time_ns,
                            "r": envelope.received_time_ns,
                            "epoch": epoch,
                            "cond": update.condition_id,
                            "tok": update.asset_id,
                            "bb": update.best_bid.as_micros(),
                            "ba": update.best_ask.as_micros(),
                        });
                        serde_json::to_writer(&mut output, &row)?;
                        output.write_all(b"\n")?;
                        *counts.entry("bba").or_default() += 1;
                    }
                    PublicMarketEvent::TickSizeChange(change) => {
                        let row = json!({
                            "k": "tick",
                            "seq": envelope.sequence,
                            "t": envelope.event_time_ns,
                            "r": envelope.received_time_ns,
                            "epoch": epoch,
                            "cond": change.condition_id,
                            "tok": change.asset_id,
                            "old": change.old_tick_size.as_micros(),
                            "new": change.new_tick_size.as_micros(),
                        });
                        serde_json::to_writer(&mut output, &row)?;
                        output.write_all(b"\n")?;
                        *counts.entry("tick").or_default() += 1;
                    }
                }
            }
            EventSource::User | EventSource::ReferencePrice | EventSource::Blockchain => {}
        }
    }

    if !reader.clean_tail() {
        return Err("journal tail incomplete; refusing to export a truncated dataset".into());
    }
    output.flush()?;

    let digest = hex(&state.digest());
    // Identical format to the order-book-replay CLI so the two can be diffed.
    println!(
        "epoch={} status={:?} sequence={:?} books={} digest={}",
        state.epoch(),
        state.status(),
        state.last_sequence(),
        state.books().len(),
        digest,
    );

    let meta = json!({
        "digest": digest,
        "epoch": state.epoch(),
        "last_sequence": state.last_sequence(),
        "books": state.books().len(),
        "depth": config.depth,
        "rows": counts,
        "output": config.output.display().to_string(),
    });
    let meta_path = config.output.with_extension("meta.json");
    let mut meta_file = File::create(&meta_path)?;
    serde_json::to_writer_pretty(&mut meta_file, &meta)?;
    meta_file.write_all(b"\n")?;
    eprintln!("meta written to {}", meta_path.display());
    Ok(())
}

fn main() -> ExitCode {
    let config = match parse_args(env::args_os().skip(1)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("export_l2 failed: {error}");
            ExitCode::FAILURE
        }
    }
}
