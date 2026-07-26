#![forbid(unsafe_code)]

use blake3::Hasher;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

const MAX_JOURNAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct SourceEvidence {
    path: String,
    bytes: u64,
    lines: u64,
    campaign_id: String,
    blake3: String,
}

#[derive(Debug, Serialize)]
struct VerifiedTrade {
    source_path: String,
    source_line: u64,
    record: Value,
    record_digest: String,
    decision: Value,
}

#[derive(Debug, Serialize)]
struct AuditDocument {
    schema_version: u16,
    sources: Vec<SourceEvidence>,
    trades: Vec<VerifiedTrade>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let output = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("output path is required")?;
    let inputs = arguments.map(PathBuf::from).collect::<Vec<_>>();
    if inputs.is_empty() {
        return Err("at least one journal is required".into());
    }
    let document = audit(&inputs)?;
    write_atomic(&output, &document)?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "sources": document.sources.len(),
            "trades": document.trades.len(),
            "output": output,
        }))?
    );
    Ok(())
}

fn audit(inputs: &[PathBuf]) -> Result<AuditDocument, Box<dyn Error>> {
    let mut sources = Vec::new();
    let mut trades = Vec::new();
    let mut trade_ids = BTreeSet::new();
    for path in inputs {
        let (source, mut source_trades) = audit_source(path, &mut trade_ids)?;
        sources.push(source);
        trades.append(&mut source_trades);
    }
    trades.sort_by_key(trade_sort_key);
    Ok(AuditDocument {
        schema_version: 1,
        sources,
        trades,
    })
}

fn audit_source(
    path: &Path,
    trade_ids: &mut BTreeSet<String>,
) -> Result<(SourceEvidence, Vec<VerifiedTrade>), Box<dyn Error>> {
    validate_journal_file(path)?;
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut line_number = 0_u64;
    let mut bytes = 0_u64;
    let mut source_hasher = Hasher::new();
    let mut expected_sequence = 1_u64;
    let mut campaign_id: Option<String> = None;
    let mut decisions = BTreeMap::<(i64, String, String), Value>::new();
    let mut trades = Vec::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if !line.ends_with('\n') {
            return Err(format!("journal has an incomplete tail: {}", path.display()).into());
        }
        line_number = line_number.saturating_add(1);
        bytes = bytes.saturating_add(u64::try_from(read)?);
        source_hasher.update(line.as_bytes());
        let (record, record_digest) = validate_record(
            &line,
            path,
            line_number,
            expected_sequence,
            &mut campaign_id,
        )?;
        expected_sequence = expected_sequence.saturating_add(1);
        collect_trade(
            path,
            line_number,
            record,
            record_digest,
            &mut decisions,
            trade_ids,
            &mut trades,
        )?;
    }
    let campaign_id = campaign_id.ok_or("journal contains no campaign")?;
    Ok((
        SourceEvidence {
            path: path.display().to_string(),
            bytes,
            lines: line_number,
            campaign_id,
            blake3: source_hasher.finalize().to_hex().to_string(),
        },
        trades,
    ))
}

fn validate_journal_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(format!("journal is not a bounded regular file: {}", path.display()).into());
    }
    Ok(())
}

fn validate_record(
    line: &str,
    path: &Path,
    line_number: u64,
    expected_sequence: u64,
    campaign_id: &mut Option<String>,
) -> Result<(Value, String), Box<dyn Error>> {
    let wrapper: Value = serde_json::from_str(line)?;
    let record = wrapper
        .get("record")
        .cloned()
        .ok_or("journal record is missing")?;
    let record_digest = wrapper
        .get("record_digest")
        .and_then(Value::as_str)
        .ok_or("journal record digest is missing")?;
    let canonical = serde_json::to_vec(&record)?;
    if blake3::hash(&canonical).to_hex().as_str() != record_digest {
        return Err(format!(
            "journal record digest mismatch at {}:{line_number}",
            path.display()
        )
        .into());
    }
    let sequence = record
        .get("sequence")
        .and_then(Value::as_u64)
        .ok_or("journal sequence is missing")?;
    if sequence != expected_sequence {
        return Err(format!(
            "journal sequence discontinuity at {}:{line_number}",
            path.display()
        )
        .into());
    }
    let current_campaign = record
        .get("campaign_id")
        .and_then(Value::as_str)
        .ok_or("journal campaign is missing")?;
    if let Some(expected) = campaign_id.as_ref() {
        if expected != current_campaign {
            return Err("journal campaign identity changed".into());
        }
    } else {
        *campaign_id = Some(current_campaign.to_owned());
    }
    Ok((record, record_digest.to_owned()))
}

fn collect_trade(
    path: &Path,
    line_number: u64,
    record: Value,
    record_digest: String,
    decisions: &mut BTreeMap<(i64, String, String), Value>,
    trade_ids: &mut BTreeSet<String>,
    trades: &mut Vec<VerifiedTrade>,
) -> Result<(), Box<dyn Error>> {
    let kind = record
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("journal kind is missing")?;
    let payload = record.get("payload").ok_or("journal payload is missing")?;
    let event_time = record
        .get("event_time_ms")
        .and_then(Value::as_i64)
        .ok_or("journal event time is missing")?;
    let asset = payload
        .get("asset")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let condition = payload
        .get("condition_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let key = (event_time, asset, condition);
    if kind == "PAIR_BUY" {
        if decisions.insert(key, record).is_some() {
            return Err("duplicate pair-buy decision".into());
        }
    } else if kind == "filled_pair" {
        let decision = decisions
            .get(&key)
            .cloned()
            .ok_or("filled pair lacks its exact pair-buy decision")?;
        let trade_id = payload
            .get("trade_id")
            .and_then(Value::as_str)
            .ok_or("filled pair trade id is missing")?;
        if !trade_ids.insert(trade_id.to_owned()) {
            return Err("duplicate paper trade id".into());
        }
        trades.push(VerifiedTrade {
            source_path: path.display().to_string(),
            source_line: line_number,
            record,
            record_digest,
            decision,
        });
    }
    Ok(())
}

fn trade_sort_key(trade: &VerifiedTrade) -> (i64, String, String) {
    (
        trade
            .record
            .get("event_time_ms")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        trade
            .record
            .get("campaign_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        trade
            .record
            .get("payload")
            .and_then(|payload| payload.get("trade_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn write_atomic(path: &Path, document: &AuditDocument) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("output has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let file = File::create(&temporary)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, document)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(sequence: u64, kind: &str) -> String {
        let payload = if kind == "PAIR_BUY" {
            json!({"asset":"BTC","condition_id":"condition","policy_id":"policy"})
        } else {
            json!({"asset":"BTC","condition_id":"condition","trade_id":"trade-1"})
        };
        let record = json!({
            "schema_version":1,
            "campaign_id":"campaign",
            "stream":"execution",
            "sequence":sequence,
            "event_time_ms":1000,
            "recorded_time_ms":1000,
            "kind":kind,
            "payload":payload,
        });
        let digest = blake3::hash(&serde_json::to_vec(&record).expect("canonical"));
        serde_json::to_string(&json!({"record":record,"record_digest":digest.to_hex().to_string()}))
            .expect("line")
            + "\n"
    }

    #[test]
    fn verifies_exact_decision_and_fill_pair() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("paper.jsonl");
        fs::write(&path, line(1, "PAIR_BUY") + &line(2, "filled_pair")).expect("fixture");
        let report = audit(&[path]).expect("audit");
        assert_eq!(report.sources.len(), 1);
        assert_eq!(report.trades.len(), 1);
    }

    #[test]
    fn rejects_digest_mismatch() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("paper.jsonl");
        let mut value: Value = serde_json::from_str(line(1, "PAIR_BUY").trim()).expect("json");
        value["record_digest"] = Value::String("0".repeat(64));
        fs::write(&path, serde_json::to_string(&value).expect("line") + "\n").expect("fixture");
        let error = audit(&[path]).expect_err("must fail");
        assert!(error.to_string().contains("digest mismatch"));
    }
}
