# BTC Per-Second Window Research Export

This local-only exporter freezes newline-complete prefixes of every configured
BTC capture. It supports exact 300-row `5m`, 900-row `15m` and 3,600-row `1h`
grids:

```sh
python3 scripts/export_btc_window_per_second.py \
  --config config/btc-5m-per-second-export.example.json

python3 scripts/export_btc_window_per_second.py \
  --config config/btc-15m-per-second-export.example.json

python3 scripts/export_btc_window_per_second.py \
  --config config/btc-1h-per-second-export.example.json
```

The atomic pointer is:

```text
var/research-export/BTC-5m-per-second/latest.json
```

It resolves to a content-addressed dataset:

```text
<revision>/
  manifest.json
  market-index.csv
  BTC-data/YYYY-MM-DD/HH/
    observations.csv
    manifest.json
```

Each five-minute market has second indices `0..299`. A row represents the state
available by the end of that UTC second. `up_observed_this_second` and
`down_observed_this_second` distinguish a new captured REST response from a
carried authentic observation. Carried values retain batch-start,
causal-availability and age fields. `market_ready=false` and `quality_flags`
make missing, stale or one-sided data explicit.

The legacy REST collector stamped the batch before sequential Up/Down requests
completed. `capture_availability_delay_ms` is therefore conservative and
policy-bound. Reducing it changes the dataset revision and requires evidence;
it must not be tuned to improve a backtest.

CSV financial values are integer micros. Full captured depth remains canonical
JSON inside the four depth columns. The export is derived research evidence and
has no capital, risk, placement, signing, submission or live authority.

## Produce one verified CSV

The sealed hourly output can be combined into a separate immutable derivative:

```sh
python3 scripts/merge_btc_window_per_second_csv.py
```

The command verifies the source pointer, root and partition manifests, all CSV
checksums, the canonical schema, fixed-point integer cells, UTC ownership,
unique condition/second keys and the exact 300-row grid before publication. It
does not modify the partitioned source.

The current merged pointer is:

```text
var/research-export/BTC-5m-per-second-merged/latest.json
```

That pointer names the content-addressed directory, single CSV and manifest.
An identical verified source revision is reused; a conflicting pre-existing
revision or corrupt partition fails closed.

For the additional windows, use the matching source and output roots:

```sh
python3 scripts/merge_btc_window_per_second_csv.py \
  --source-latest var/research-export/BTC-15m-per-second/latest.json \
  --source-root var/research-export/BTC-15m-per-second \
  --output-root var/research-export/BTC-15m-per-second-merged

python3 scripts/merge_btc_window_per_second_csv.py \
  --source-latest var/research-export/BTC-1h-per-second/latest.json \
  --source-root var/research-export/BTC-1h-per-second \
  --output-root var/research-export/BTC-1h-per-second-merged
```
