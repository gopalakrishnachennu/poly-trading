#![forbid(unsafe_code)]

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

const EXPORT_ROOT_ENV: &str = "POLY_RESEARCH_EXPORT_DIR";
const MAX_JOURNAL_BYTES: u64 = 512 * 1024 * 1024;
const EXPORT_SCHEMA_VERSION: u16 = 1;

const OBSERVATION_COLUMNS: &[&str] = &[
    "event_time_ms",
    "recorded_time_ms",
    "sequence",
    "record_digest",
    "campaign_id",
    "runtime_config_id",
    "runtime_config_digest",
    "asset",
    "condition_id",
    "reference_price_micros",
    "target_price_micros",
    "up_best_bid_micros",
    "up_best_ask_micros",
    "down_best_bid_micros",
    "down_best_ask_micros",
    "feed_age_ms",
];
const DECISION_COLUMNS: &[&str] = &[
    "event_time_ms",
    "recorded_time_ms",
    "sequence",
    "record_digest",
    "campaign_id",
    "runtime_config_id",
    "runtime_config_digest",
    "asset",
    "condition_id",
    "decision",
    "reason",
    "pair_cost_micros",
    "fee_micros",
    "slippage_micros",
    "net_cost_micros",
    "quantity_micros",
    "policy_id",
    "policy_digest",
];
const TRADE_COLUMNS: &[&str] = &[
    "event_time_ms",
    "recorded_time_ms",
    "sequence",
    "record_digest",
    "campaign_id",
    "runtime_config_id",
    "runtime_config_digest",
    "asset",
    "condition_id",
    "trade_id",
    "state",
    "quantity_micros",
    "up_price_micros",
    "down_price_micros",
    "fee_micros",
    "slippage_micros",
    "cost_micros",
    "locked_pnl_micros",
    "decision_at_ms",
];

#[derive(Clone, Debug, Default, Serialize)]
pub struct ResearchExportStatus {
    pub available: bool,
    pub root: String,
    pub last_exported_at_ms: Option<i64>,
    pub campaign_id: Option<String>,
    pub source_records: u64,
    pub partitions: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResearchExportReport {
    pub status: ResearchExportStatus,
    pub observation_rows: u64,
    pub decision_rows: u64,
    pub trade_rows: u64,
}

#[derive(Debug, Default)]
pub struct ResearchExporter {
    status: ResearchExportStatus,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PartitionKey {
    asset: String,
    utc_date: String,
    utc_hour: String,
}

#[derive(Default)]
struct PartitionRows {
    observations: Vec<BTreeMap<String, String>>,
    decisions: Vec<BTreeMap<String, String>>,
    trades: Vec<BTreeMap<String, String>>,
    source_sequences: BTreeSet<u64>,
    source_digests: Vec<String>,
}

impl ResearchExporter {
    #[must_use]
    pub fn status(&self) -> ResearchExportStatus {
        self.status.clone()
    }

    pub fn refresh(
        &mut self,
        journal_path: &Path,
        exported_at_ms: i64,
    ) -> Result<ResearchExportReport, String> {
        let root = export_root();
        let result = export_verified(journal_path, &root, exported_at_ms);
        match result {
            Ok(report) => {
                self.status = report.status.clone();
                Ok(report)
            }
            Err(error) => {
                self.status = ResearchExportStatus {
                    available: false,
                    root: root.display().to_string(),
                    last_error: Some(error.clone()),
                    ..self.status.clone()
                };
                Err(error)
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn export_verified(
    journal_path: &Path,
    root: &Path,
    exported_at_ms: i64,
) -> Result<ResearchExportReport, String> {
    if exported_at_ms < 0 {
        return Err("export timestamp is invalid".into());
    }
    let metadata =
        fs::metadata(journal_path).map_err(|error| format!("journal metadata failed: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return Err("journal path is not a bounded regular file".into());
    }
    let source = fs::read_to_string(journal_path)
        .map_err(|error| format!("journal read failed: {error}"))?;
    if source.is_empty() {
        return Err("journal is empty".into());
    }
    let mut expected_sequence = 1_u64;
    let mut campaign_id: Option<String> = None;
    let mut partitions = BTreeMap::<PartitionKey, PartitionRows>::new();
    for line in source.lines() {
        let envelope: Value =
            serde_json::from_str(line).map_err(|error| format!("journal JSON invalid: {error}"))?;
        let record = envelope
            .get("record")
            .cloned()
            .ok_or("journal record missing")?;
        let digest = envelope
            .get("record_digest")
            .and_then(Value::as_str)
            .ok_or("journal digest missing")?
            .to_owned();
        let canonical = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        if hex(blake3::hash(&canonical).as_bytes()) != digest {
            return Err("journal digest mismatch".into());
        }
        let sequence = record
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or("journal sequence missing")?;
        if sequence != expected_sequence {
            return Err("journal sequence discontinuity".into());
        }
        expected_sequence = expected_sequence.saturating_add(1);
        let record_campaign = record
            .get("campaign_id")
            .and_then(Value::as_str)
            .ok_or("journal campaign identity missing")?
            .to_owned();
        if let Some(expected_campaign) = &campaign_id {
            if expected_campaign != &record_campaign {
                return Err("journal campaign identity changed".into());
            }
        } else {
            campaign_id = Some(record_campaign.clone());
        }
        let kind = record
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("journal kind missing")?;
        let payload = record.get("payload").ok_or("journal payload missing")?;
        let Some(asset) = payload
            .get("asset")
            .and_then(Value::as_str)
            .map(normalize_asset)
            .transpose()?
        else {
            continue;
        };
        let event_time_ms = record
            .get("event_time_ms")
            .and_then(Value::as_i64)
            .ok_or("journal event timestamp missing")?;
        let key = partition_key(&asset, event_time_ms)?;
        let rows = partitions.entry(key).or_default();
        rows.source_sequences.insert(sequence);
        rows.source_digests.push(digest.clone());
        let row = base_row(&record, &digest, sequence, &record_campaign);
        match kind {
            "observation" => {
                rows.observations
                    .push(merge_payload(row, payload, OBSERVATION_COLUMNS));
            }
            "PAIR_BUY" | "NO_TRADE" => {
                let mut decision = merge_payload(row, payload, DECISION_COLUMNS);
                decision.insert("decision".into(), kind.into());
                rows.decisions.push(decision);
            }
            "filled_pair" => rows.trades.push(merge_payload(row, payload, TRADE_COLUMNS)),
            _ => {}
        }
    }
    let campaign_id = campaign_id.ok_or("journal contains no campaign")?;
    let mut observation_rows = 0_u64;
    let mut decision_rows = 0_u64;
    let mut trade_rows = 0_u64;
    for (key, rows) in &partitions {
        let directory = root
            .join(format!("{}-data", key.asset))
            .join(&key.utc_date)
            .join(&key.utc_hour);
        fs::create_dir_all(&directory)
            .map_err(|error| format!("export directory failed: {error}"))?;
        observation_rows = observation_rows
            .saturating_add(u64::try_from(rows.observations.len()).unwrap_or(u64::MAX));
        decision_rows =
            decision_rows.saturating_add(u64::try_from(rows.decisions.len()).unwrap_or(u64::MAX));
        trade_rows =
            trade_rows.saturating_add(u64::try_from(rows.trades.len()).unwrap_or(u64::MAX));
        write_dataset(
            &directory,
            "observations",
            OBSERVATION_COLUMNS,
            &rows.observations,
        )?;
        write_dataset(&directory, "decisions", DECISION_COLUMNS, &rows.decisions)?;
        write_dataset(&directory, "trades", TRADE_COLUMNS, &rows.trades)?;
        let manifest = json!({
            "schema_version": EXPORT_SCHEMA_VERSION,
            "campaign_id": campaign_id,
            "asset": key.asset,
            "utc_date": key.utc_date,
            "utc_hour": key.utc_hour,
            "source_sequence_start": rows.source_sequences.first(),
            "source_sequence_end": rows.source_sequences.last(),
            "source_record_digests": rows.source_digests,
            "row_counts": {"observations": rows.observations.len(), "decisions": rows.decisions.len(), "trades": rows.trades.len()},
            "files": file_checksums(&directory)?,
            "exported_at_ms": exported_at_ms,
            "derived_not_financial_authority": true,
        });
        write_atomic(
            &directory.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(ResearchExportReport {
        status: ResearchExportStatus {
            available: true,
            root: root.display().to_string(),
            last_exported_at_ms: Some(exported_at_ms),
            campaign_id: Some(campaign_id),
            source_records: expected_sequence.saturating_sub(1),
            partitions: u64::try_from(partitions.len()).unwrap_or(u64::MAX),
            last_error: None,
        },
        observation_rows,
        decision_rows,
        trade_rows,
    })
}

fn base_row(
    record: &Value,
    digest: &str,
    sequence: u64,
    campaign_id: &str,
) -> BTreeMap<String, String> {
    let mut row = BTreeMap::new();
    row.insert(
        "event_time_ms".into(),
        value_text(record.get("event_time_ms")),
    );
    row.insert(
        "recorded_time_ms".into(),
        value_text(record.get("recorded_time_ms")),
    );
    row.insert("sequence".into(), sequence.to_string());
    row.insert("record_digest".into(), digest.into());
    row.insert("campaign_id".into(), campaign_id.into());
    row.insert(
        "runtime_config_id".into(),
        value_text(record.get("runtime_config_id")),
    );
    row.insert(
        "runtime_config_digest".into(),
        value_text(record.get("runtime_config_digest")),
    );
    row
}

fn merge_payload(
    mut row: BTreeMap<String, String>,
    payload: &Value,
    columns: &[&str],
) -> BTreeMap<String, String> {
    for column in columns {
        if !row.contains_key(*column) {
            row.insert((*column).into(), value_text(payload.get(*column)));
        }
    }
    row
}

fn partition_key(asset: &str, event_time_ms: i64) -> Result<PartitionKey, String> {
    let date_time = chrono::DateTime::from_timestamp_millis(event_time_ms)
        .ok_or("journal event time out of range")?;
    Ok(PartitionKey {
        asset: asset.into(),
        utc_date: date_time.format("%Y-%m-%d").to_string(),
        utc_hour: date_time.format("%H").to_string(),
    })
}

fn normalize_asset(raw: &str) -> Result<String, String> {
    let asset = raw.trim().to_ascii_uppercase();
    if !(2..=10).contains(&asset.len())
        || !asset
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("journal asset is invalid".into());
    }
    Ok(asset)
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn write_dataset(
    directory: &Path,
    name: &str,
    columns: &[&str],
    rows: &[BTreeMap<String, String>],
) -> Result<(), String> {
    write_csv(&directory.join(format!("{name}.csv")), columns, rows)?;
    write_parquet(&directory.join(format!("{name}.parquet")), columns, rows)
}

fn write_csv(
    path: &Path,
    columns: &[&str],
    rows: &[BTreeMap<String, String>],
) -> Result<(), String> {
    let mut output = Vec::new();
    output.extend_from_slice(columns.join(",").as_bytes());
    output.push(b'\n');
    for row in rows {
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                output.push(b',');
            }
            csv_cell(&mut output, row.get(*column).map_or("", String::as_str));
        }
        output.push(b'\n');
    }
    write_atomic(path, &output)
}

fn csv_cell(output: &mut Vec<u8>, value: &str) {
    if value.contains([',', '"', '\n', '\r']) {
        output.push(b'"');
        for byte in value.bytes() {
            output.push(byte);
            if byte == b'"' {
                output.push(b'"');
            }
        }
        output.push(b'"');
    } else {
        output.extend_from_slice(value.as_bytes());
    }
}

fn write_parquet(
    path: &Path,
    columns: &[&str],
    rows: &[BTreeMap<String, String>],
) -> Result<(), String> {
    let fields = columns
        .iter()
        .map(|column| Field::new(*column, DataType::Utf8, false))
        .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new(fields));
    let arrays = columns
        .iter()
        .map(|column| {
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.get(*column).map_or("", String::as_str))
                    .collect::<Vec<_>>(),
            )) as ArrayRef
        })
        .collect::<Vec<_>>();
    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|error| format!("Parquet batch failed: {error}"))?;
    let temporary = temporary_path(path);
    let file =
        File::create(&temporary).map_err(|error| format!("Parquet create failed: {error}"))?;
    let mut writer = ArrowWriter::try_new(file, schema, None)
        .map_err(|error| format!("Parquet writer failed: {error}"))?;
    writer
        .write(&batch)
        .map_err(|error| format!("Parquet write failed: {error}"))?;
    writer
        .close()
        .map_err(|error| format!("Parquet close failed: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("Parquet publish failed: {error}"))
}

fn file_checksums(directory: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut checksums = BTreeMap::new();
    for name in [
        "observations.csv",
        "observations.parquet",
        "decisions.csv",
        "decisions.parquet",
        "trades.csv",
        "trades.parquet",
    ] {
        let bytes = fs::read(directory.join(name))
            .map_err(|error| format!("derived file checksum failed: {error}"))?;
        checksums.insert(name.into(), hex(blake3::hash(&bytes).as_bytes()));
    }
    Ok(checksums)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = temporary_path(path);
    let mut file =
        File::create(&temporary).map_err(|error| format!("derived file create failed: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("derived file write failed: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("derived file sync failed: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("derived file publish failed: {error}"))
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("data"),
        std::process::id()
    ))
}

fn export_root() -> PathBuf {
    PathBuf::from(env::var(EXPORT_ROOT_ENV).unwrap_or_else(|_| "var/research-export".into()))
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push(char::from(TABLE[usize::from(byte >> 4)]));
        rendered.push(char::from(TABLE[usize::from(byte & 0x0f)]));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{export_verified, hex};
    use serde_json::json;
    use std::fs;

    fn journal_line(sequence: u64, kind: &str, asset: &str, event_time_ms: i64) -> String {
        let payload = match kind {
            "observation" => json!({
                "asset": asset,
                "condition_id": "condition",
                "reference_price_micros": "1000000",
                "target_price_micros": "999000",
                "up_best_bid_micros": "490000",
                "up_best_ask_micros": "500000",
                "down_best_bid_micros": "490000",
                "down_best_ask_micros": "500000",
                "feed_age_ms": 3,
            }),
            "filled_pair" => json!({
                "asset": asset,
                "condition_id": "condition",
                "trade_id": "trade-1",
                "state": "FILLED_PAIR_LOCKED",
                "quantity_micros": "1000000",
                "up_price_micros": "490000",
                "down_price_micros": "490000",
                "fee_micros": "1000",
                "slippage_micros": "500",
                "cost_micros": "981500",
                "locked_pnl_micros": "18500",
                "decision_at_ms": event_time_ms,
            }),
            _ => json!({
                "asset": asset,
                "condition_id": "condition",
                "reason": "test",
                "pair_cost_micros": "980000",
                "fee_micros": "1000",
                "slippage_micros": "500",
                "net_cost_micros": "981500",
                "quantity_micros": "1000000",
                "policy_id": "policy",
                "policy_digest": "a".repeat(64),
            }),
        };
        let record = json!({
            "schema_version": 1,
            "campaign_id": "paper-test",
            "runtime_config_id": "runtime",
            "runtime_config_digest": "b".repeat(64),
            "stream": "test",
            "sequence": sequence,
            "event_time_ms": event_time_ms,
            "recorded_time_ms": event_time_ms,
            "kind": kind,
            "payload": payload,
        });
        let canonical = serde_json::to_vec(&record).expect("canonical record");
        serde_json::to_string(
            &json!({"record": record, "record_digest": hex(blake3::hash(&canonical).as_bytes())}),
        )
        .expect("journal line")
    }

    #[test]
    fn verified_journal_exports_csv_parquet_and_manifest_per_asset_hour() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let journal = temporary.path().join("paper.jsonl");
        let hour = 1_784_678_400_000_i64;
        fs::write(
            &journal,
            [
                journal_line(1, "observation", "BTC", hour),
                journal_line(2, "NO_TRADE", "BTC", hour + 1),
                journal_line(3, "filled_pair", "ETH", hour + 2),
            ]
            .join("\n"),
        )
        .expect("journal write");
        let root = temporary.path().join("research");
        let report = export_verified(&journal, &root, hour + 3).expect("export succeeds");
        assert_eq!(report.status.partitions, 2);
        assert_eq!(report.observation_rows, 1);
        assert_eq!(report.decision_rows, 1);
        assert_eq!(report.trade_rows, 1);
        let btc = root.join("BTC-data/2026-07-22/00");
        assert!(btc.join("observations.csv").is_file());
        assert!(btc.join("observations.parquet").is_file());
        assert!(btc.join("manifest.json").is_file());
    }

    #[test]
    fn corrupt_journal_never_creates_an_export() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let journal = temporary.path().join("paper.jsonl");
        fs::write(&journal, "{\"record\":{},\"record_digest\":\"bad\"}\n").expect("journal write");
        let root = temporary.path().join("research");
        assert!(export_verified(&journal, &root, 1_784_678_400_000).is_err());
        assert!(!root.exists());
    }
}
