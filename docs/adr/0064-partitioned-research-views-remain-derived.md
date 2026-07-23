# ADR 0064: Export partitioned research views from verified paper journals

- Status: Accepted
- Date: 2026-07-23

## Decision

The paper journal remains the only authoritative local evidence source. A
manual, local-only exporter first verifies the bounded JSONL journal's BLAKE3
record digests, campaign identity and contiguous sequence. It then writes
derived CSV and Parquet files partitioned by asset, UTC date and UTC hour:

```text
var/research-export/BTC-data/YYYY-MM-DD/HH/
var/research-export/ETH-data/YYYY-MM-DD/HH/
```

Every partition includes a manifest containing the campaign identity, source
sequence range, source record digests, row counts, exported-file checksums and
an explicit non-authoritative marker. Fixed-point values remain decimal strings
in both formats. Individual derived files are atomically replaced; the manifest
is published only after every data file is available.

## Consequences

- Operators can inspect data in a spreadsheet through CSV while research tools
  can efficiently read Parquet without changing the source journal.
- Corruption, a sequence gap, a campaign mismatch or invalid timestamp stops
  the entire export before any partition is created.
- The browser can display export status and request a local refresh, but has no
  upload, credential, order, wallet, signing or financial-authority path.
