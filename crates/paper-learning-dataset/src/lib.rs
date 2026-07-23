#![forbid(unsafe_code)]

//! Freezes verified paper-journal data into chronological research folds.
//!
//! This crate performs no training and has no network, model inference,
//! credentials, signing, wallet, capital, risk, placement or submission path.

use model_governance::{ModelArtifact, WalkForwardFold, WalkForwardPlan};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};
use thiserror::Error;

pub const DATASET_SCHEMA_VERSION: u16 = 1;
const MAX_JOURNAL_BYTES: usize = 512 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000_000;
const MAX_STREAM_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetPolicy {
    pub maximum_journal_bytes: usize,
    pub maximum_records: usize,
    pub train_bps: u16,
    pub validation_bps: u16,
    pub minimum_records_per_fold: usize,
    pub policy_digest: [u8; 32],
}

impl DatasetPolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.policy_digest = digest_without(b"paper-dataset-policy-v1", &self, |value| {
            value.policy_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest
            == digest_without(b"paper-dataset-policy-v1", self, |value| {
                value.policy_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetRecord {
    pub sequence: u64,
    pub stream: String,
    pub kind: String,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub strategy_available_time_ns: i64,
    pub payload_digest: [u8; 32],
    pub record_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldManifest {
    pub fold: WalkForwardFold,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub records: u64,
    pub records_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenDataset {
    pub schema_version: u16,
    pub campaign_id: String,
    pub source_journal_digest: [u8; 32],
    pub policy_digest: [u8; 32],
    pub records: Vec<DatasetRecord>,
    pub train: FoldManifest,
    pub validation: FoldManifest,
    pub test: FoldManifest,
    pub plan: WalkForwardPlan,
    pub dataset_digest: [u8; 32],
}

impl FrozenDataset {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.dataset_digest
            == digest_without(b"frozen-paper-dataset-v1", self, |value| {
                value.dataset_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ArtifactSubmission {
    pub model_id: model_governance::ModelId,
    pub dataset_digest: [u8; 32],
    pub train_fold_digest: [u8; 32],
    pub validation_fold_digest: [u8; 32],
    pub test_fold_digest: [u8; 32],
    pub artifact_digest: [u8; 32],
    pub paper_only: bool,
    pub capital_authority: bool,
    pub risk_authority: bool,
    pub placement_authority: bool,
    pub signing_authority: bool,
    pub submission_authority: bool,
    pub submission_digest: [u8; 32],
}

impl ArtifactSubmission {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.submission_digest
            == digest_without(b"paper-artifact-submission-v1", self, |value| {
                value.submission_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DatasetError {
    #[error("dataset policy invalid")]
    Policy,
    #[error("journal read failed: {0}")]
    Read(String),
    #[error("journal is oversized or record count exceeds policy")]
    Bounds,
    #[error("journal record encoding invalid")]
    Encoding,
    #[error("journal record digest invalid")]
    Digest,
    #[error("journal campaign, stream, timestamp or sequence invalid")]
    Journal,
    #[error("insufficient distinct chronological availability buckets")]
    Chronology,
    #[error("dataset or artifact provenance invalid")]
    Provenance,
}

/// Reads and freezes one complete paper journal.
///
/// # Errors
///
/// Returns an error for any I/O, bounds, digest, campaign, timestamp,
/// chronology or policy violation. It never silently skips a bad record.
pub fn freeze_jsonl_path(
    path: &Path,
    policy: &DatasetPolicy,
) -> Result<FrozenDataset, DatasetError> {
    let bytes = fs::read(path).map_err(|error| DatasetError::Read(error.to_string()))?;
    freeze_jsonl(&bytes, policy)
}

/// Freezes one JSONL journal byte stream into immutable chronological folds.
///
/// # Errors
///
/// Returns an error for malformed or unverifiable journal records, invalid
/// policy or insufficient chronology for safe walk-forward separation.
pub fn freeze_jsonl(bytes: &[u8], policy: &DatasetPolicy) -> Result<FrozenDataset, DatasetError> {
    validate_policy(policy)?;
    if bytes.len() > policy.maximum_journal_bytes || bytes.len() > MAX_JOURNAL_BYTES {
        return Err(DatasetError::Bounds);
    }
    let mut campaign_id: Option<String> = None;
    let mut records = Vec::new();
    let mut seen_digests = BTreeSet::new();
    let mut expected_sequence = 1_u64;
    let mut source_hasher = blake3::Hasher::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if records.len() >= policy.maximum_records || records.len() >= MAX_RECORDS {
            return Err(DatasetError::Bounds);
        }
        let wrapper: JournalLine =
            serde_json::from_slice(line).map_err(|_| DatasetError::Encoding)?;
        let record_bytes =
            serde_json::to_vec(&wrapper.record).map_err(|_| DatasetError::Encoding)?;
        let record_digest = *blake3::hash(&record_bytes).as_bytes();
        if decode_hex_digest(&wrapper.record_digest)? != record_digest
            || !seen_digests.insert(record_digest)
        {
            return Err(DatasetError::Digest);
        }
        let record: RawRecord =
            serde_json::from_value(wrapper.record).map_err(|_| DatasetError::Encoding)?;
        if record.schema_version != 1
            || record.sequence != expected_sequence
            || record.campaign_id.is_empty()
            || record.stream.is_empty()
            || record.stream.len() > MAX_STREAM_BYTES
            || record.kind.is_empty()
            || record.kind.len() > MAX_STREAM_BYTES
            || record.event_time_ms < 0
            || record.recorded_time_ms < record.event_time_ms
        {
            return Err(DatasetError::Journal);
        }
        if campaign_id
            .as_ref()
            .is_some_and(|value| value != &record.campaign_id)
        {
            return Err(DatasetError::Journal);
        }
        campaign_id.get_or_insert_with(|| record.campaign_id.clone());
        let event_time_ns = millis_to_ns(record.event_time_ms)?;
        let received_time_ns = millis_to_ns(record.recorded_time_ms)?;
        let payload_bytes =
            serde_json::to_vec(&record.payload).map_err(|_| DatasetError::Encoding)?;
        records.push(DatasetRecord {
            sequence: record.sequence,
            stream: record.stream,
            kind: record.kind,
            event_time_ns,
            received_time_ns,
            // A paper record becomes usable at its local recorded timestamp.
            // It is deliberately never advanced to a later corrected value.
            strategy_available_time_ns: received_time_ns,
            payload_digest: *blake3::hash(&payload_bytes).as_bytes(),
            record_digest,
        });
        source_hasher.update(line);
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(DatasetError::Journal)?;
    }
    let campaign_id = campaign_id.ok_or(DatasetError::Journal)?;
    build_dataset(
        campaign_id,
        *source_hasher.finalize().as_bytes(),
        policy,
        records,
    )
}

/// Binds a frozen research artifact to the immutable dataset it was trained on.
///
/// # Errors
///
/// Returns an error for tampered dataset/artifact provenance or for a model
/// artifact whose training/validation folds do not exactly match the dataset.
pub fn bind_artifact(
    dataset: &FrozenDataset,
    artifact: &ModelArtifact,
) -> Result<ArtifactSubmission, DatasetError> {
    if !dataset.verify_digest()
        || !dataset.plan.verify_digest()
        || !artifact.verify_digest()
        || artifact.training_fold_digest != dataset.train.fold.fold_digest
        || artifact.validation_fold_digest != dataset.validation.fold.fold_digest
        || artifact.frozen_at_ns > dataset.test.fold.start_available_time_ns
    {
        return Err(DatasetError::Provenance);
    }
    Ok(seal_submission(ArtifactSubmission {
        model_id: artifact.model_id,
        dataset_digest: dataset.dataset_digest,
        train_fold_digest: dataset.train.fold.fold_digest,
        validation_fold_digest: dataset.validation.fold.fold_digest,
        test_fold_digest: dataset.test.fold.fold_digest,
        artifact_digest: artifact.artifact_digest,
        paper_only: true,
        capital_authority: false,
        risk_authority: false,
        placement_authority: false,
        signing_authority: false,
        submission_authority: false,
        submission_digest: [0; 32],
    }))
}

fn build_dataset(
    campaign_id: String,
    source_journal_digest: [u8; 32],
    policy: &DatasetPolicy,
    mut records: Vec<DatasetRecord>,
) -> Result<FrozenDataset, DatasetError> {
    records.sort_by_key(|record| (record.strategy_available_time_ns, record.sequence));
    let mut buckets = Vec::new();
    let mut start = 0_usize;
    while start < records.len() {
        let time = records[start].strategy_available_time_ns;
        let mut end = start + 1;
        while end < records.len() && records[end].strategy_available_time_ns == time {
            end += 1;
        }
        buckets.push((start, end));
        start = end;
    }
    if buckets.len() < 3 {
        return Err(DatasetError::Chronology);
    }
    let bucket_count = buckets.len();
    let train_buckets =
        (bucket_count * usize::from(policy.train_bps) / 10_000).clamp(1, bucket_count - 2);
    let remaining_after_train = bucket_count - train_buckets;
    let validation_share = usize::from(policy.validation_bps) * remaining_after_train
        / (10_000 - usize::from(policy.train_bps));
    let validation_buckets = validation_share.clamp(1, remaining_after_train - 1);
    let train_end = buckets[train_buckets - 1].1;
    let validation_end = buckets[train_buckets + validation_buckets - 1].1;
    let train = fold_manifest(&records[..train_end], model_governance::FoldKind::Train)?;
    let validation = fold_manifest(
        &records[train_end..validation_end],
        model_governance::FoldKind::Validation,
    )?;
    let test = fold_manifest(&records[validation_end..], model_governance::FoldKind::Test)?;
    if [train.records, validation.records, test.records]
        .iter()
        .any(|records| {
            *records < u64::try_from(policy.minimum_records_per_fold).unwrap_or(u64::MAX)
        })
    {
        return Err(DatasetError::Chronology);
    }
    let plan = WalkForwardPlan {
        train: train.fold.clone(),
        validation: validation.fold.clone(),
        test: test.fold.clone(),
        plan_digest: [0; 32],
    }
    .sealed();
    let mut dataset = FrozenDataset {
        schema_version: DATASET_SCHEMA_VERSION,
        campaign_id,
        source_journal_digest,
        policy_digest: policy.policy_digest,
        records,
        train,
        validation,
        test,
        plan,
        dataset_digest: [0; 32],
    };
    dataset.dataset_digest = digest_without(b"frozen-paper-dataset-v1", &dataset, |value| {
        value.dataset_digest = [0; 32];
    });
    Ok(dataset)
}

fn fold_manifest(
    records: &[DatasetRecord],
    kind: model_governance::FoldKind,
) -> Result<FoldManifest, DatasetError> {
    let first = records.first().ok_or(DatasetError::Chronology)?;
    let last = records.last().ok_or(DatasetError::Chronology)?;
    let digest = digest_records(records);
    let fold = WalkForwardFold {
        kind,
        start_available_time_ns: first.strategy_available_time_ns,
        end_available_time_ns: last
            .strategy_available_time_ns
            .checked_add(1)
            .ok_or(DatasetError::Chronology)?,
        data_digest: digest,
        fold_digest: [0; 32],
    }
    .sealed();
    Ok(FoldManifest {
        fold,
        first_sequence: first.sequence,
        last_sequence: last.sequence,
        records: u64::try_from(records.len()).map_err(|_| DatasetError::Bounds)?,
        records_digest: digest,
    })
}

fn validate_policy(policy: &DatasetPolicy) -> Result<(), DatasetError> {
    if !policy.verify_digest()
        || policy.maximum_journal_bytes == 0
        || policy.maximum_journal_bytes > MAX_JOURNAL_BYTES
        || policy.maximum_records == 0
        || policy.maximum_records > MAX_RECORDS
        || policy.train_bps == 0
        || policy.validation_bps == 0
        || usize::from(policy.train_bps) + usize::from(policy.validation_bps) >= 10_000
        || policy.minimum_records_per_fold == 0
    {
        return Err(DatasetError::Policy);
    }
    Ok(())
}

fn millis_to_ns(millis: i64) -> Result<i64, DatasetError> {
    millis.checked_mul(1_000_000).ok_or(DatasetError::Journal)
}

fn digest_records(records: &[DatasetRecord]) -> [u8; 32] {
    let bytes = serde_json::to_vec(records).expect("bounded dataset serialization");
    *blake3::hash(&bytes).as_bytes()
}

fn seal_submission(mut submission: ArtifactSubmission) -> ArtifactSubmission {
    submission.submission_digest =
        digest_without(b"paper-artifact-submission-v1", &submission, |value| {
            value.submission_digest = [0; 32];
        });
    submission
}

fn decode_hex_digest(value: &str) -> Result<[u8; 32], DatasetError> {
    if value.len() != 64 {
        return Err(DatasetError::Digest);
    }
    let mut result = [0_u8; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        let offset = index * 2;
        let high = value.as_bytes()[offset];
        let low = value.as_bytes()[offset + 1];
        *byte = (hex_value(high)? << 4) | hex_value(low)?;
    }
    Ok(result)
}

fn hex_value(value: u8) -> Result<u8, DatasetError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(DatasetError::Digest),
    }
}

fn digest_without<T: Serialize + Clone>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut copy = value.clone();
    clear(&mut copy);
    let bytes = serde_json::to_vec(&copy).expect("bounded dataset serialization");
    *blake3::hash(&[domain, &bytes].concat()).as_bytes()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalLine {
    record: Value,
    record_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    schema_version: u16,
    campaign_id: String,
    stream: String,
    sequence: u64,
    event_time_ms: i64,
    recorded_time_ms: i64,
    kind: String,
    payload: Value,
}

#[cfg(test)]
mod tests;
