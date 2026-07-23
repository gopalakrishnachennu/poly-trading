# Phase 4.5 — Replay-Verified Partitioned Research Export

## Objective

Produce human-readable CSV and columnar Parquet research views from the
authoritative paper JSONL journal without treating either view as financial
authority.

## Acceptance criteria

1. Export verifies every JSONL record's BLAKE3 digest, campaign identity, and
   contiguous sequence before writing any derived partition.
2. Rows are partitioned by asset / UTC date / UTC hour and emitted in both CSV
   and Parquet with fixed-point values represented as decimal strings.
3. Each partition contains a manifest binding campaign ID, source journal
   digest/range, row counts, and derived-file checksums.
4. Existing derived data is replaced atomically; corrupt journals write no
   output and report failure.
5. The terminal exposes export status and an explicit local refresh action;
   it has no upload, credential, signing, or live-trading capability.
