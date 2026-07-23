use super::*;
use model_governance::ModelId;

fn policy() -> DatasetPolicy {
    DatasetPolicy {
        maximum_journal_bytes: 1_000_000,
        maximum_records: 100,
        train_bps: 6_000,
        validation_bps: 2_000,
        minimum_records_per_fold: 1,
        policy_digest: [0; 32],
    }
    .sealed()
}

fn hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}

fn journal(records: &[(u64, i64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (sequence, time) in records {
        let record = serde_json::json!({"schema_version":1,"campaign_id":"paper-test","stream":"market","sequence":sequence,"event_time_ms":time,"recorded_time_ms":time,"kind":"observation","payload":{"reference":sequence}});
        let bytes = serde_json::to_vec(&record).unwrap();
        let line = serde_json::json!({"record":record,"record_digest":hex(blake3::hash(&bytes).as_bytes())});
        out.extend(serde_json::to_vec(&line).unwrap());
        out.push(b'\n');
    }
    out
}

#[test]
fn freezes_disjoint_chronological_folds_and_binds_artifact() {
    let frozen = freeze_jsonl(
        &journal(&[(1, 10), (2, 10), (3, 20), (4, 30), (5, 40), (6, 50)]),
        &policy(),
    )
    .unwrap();
    assert!(frozen.verify_digest());
    assert!(frozen.plan.verify_digest());
    assert!(
        frozen.train.fold.end_available_time_ns <= frozen.validation.fold.start_available_time_ns
    );
    assert!(
        frozen.validation.fold.end_available_time_ns <= frozen.test.fold.start_available_time_ns
    );
    let artifact = ModelArtifact {
        model_id: ModelId([1; 32]),
        label: "frozen-model".into(),
        training_fold_digest: frozen.train.fold.fold_digest,
        validation_fold_digest: frozen.validation.fold.fold_digest,
        feature_schema_digest: [2; 32],
        configuration_digest: [3; 32],
        code_digest: [4; 32],
        trained_at_ns: frozen.train.fold.end_available_time_ns,
        frozen_at_ns: frozen.test.fold.start_available_time_ns,
        artifact_digest: [0; 32],
    }
    .sealed();
    let submission = bind_artifact(&frozen, &artifact).unwrap();
    assert!(submission.verify_digest());
    assert!(submission.paper_only);
    assert!(!submission.submission_authority);
}

#[test]
fn corrupt_duplicate_and_mixed_campaign_records_fail_closed() {
    let mut bytes = journal(&[(1, 10), (2, 20), (3, 30)]);
    bytes[20] ^= 1;
    assert!(matches!(
        freeze_jsonl(&bytes, &policy()),
        Err(DatasetError::Encoding | DatasetError::Digest)
    ));
    assert_eq!(
        freeze_jsonl(&journal(&[(1, 10), (1, 20), (3, 30)]), &policy()),
        Err(DatasetError::Journal)
    );
}

#[test]
fn insufficient_availability_buckets_and_wrong_artifact_binding_fail() {
    assert_eq!(
        freeze_jsonl(&journal(&[(1, 10), (2, 10)]), &policy()),
        Err(DatasetError::Chronology)
    );
    let frozen = freeze_jsonl(&journal(&[(1, 10), (2, 20), (3, 30), (4, 40)]), &policy()).unwrap();
    let artifact = ModelArtifact {
        model_id: ModelId([1; 32]),
        label: "wrong".into(),
        training_fold_digest: [9; 32],
        validation_fold_digest: frozen.validation.fold.fold_digest,
        feature_schema_digest: [2; 32],
        configuration_digest: [3; 32],
        code_digest: [4; 32],
        trained_at_ns: frozen.train.fold.end_available_time_ns,
        frozen_at_ns: frozen.test.fold.start_available_time_ns,
        artifact_digest: [0; 32],
    }
    .sealed();
    assert_eq!(
        bind_artifact(&frozen, &artifact),
        Err(DatasetError::Provenance)
    );
}
