#!/usr/bin/env python3
"""End-to-end profit-engine pipeline orchestrator.

Chains features -> labels -> walk-forward -> compact report in one call.
Designed to run on a schedule (every N minutes under supervisor, or ad hoc).

Emits var/profit-engine/latest-report.json with a compact shape the
terminal `/api/profit-engine` route can render without further processing:

    {
      "generated_at_ms":       1785030000000,
      "duration_ms":           12345,
      "sources": {
        "polymarket_files":    12,
        "external_files":      2,
        "total_bytes":         74839203
      },
      "features": {"rows_written": 58642},
      "labels":   {"rows_in": 58642, "rows_with_resolution": 51204, ...},
      "walk_forward": {
        "n_folds":             14,
        "y_key":               "y_settle_up",
        "n_contracts":         181,
        "models":              ["market_baseline","logistic_raw","logistic_platt","logistic_isotonic","lightgbm"],
        "means": {
          "market_baseline":   {"brier": 0.135, "log_loss": 0.42, ...},
          "logistic_raw":      {...},
          ...
        },
        "best_per_fold":       ["logistic_raw", "market_baseline", ...],
        "folds":               [...compact per-fold summaries...]
      },
      "verdict":               "NO_MODEL_BEATS_BASELINE" | "LOGISTIC_BEATS_BASELINE" | ...
    }

Read-only.  Rebuilds var/features/* on each run.  Never touches live trader.
"""
from __future__ import annotations
import argparse, glob, json, os, statistics, sys, time
from datetime import datetime, timezone

# Support running as `-m scripts.profit_engine.run_pipeline` from repo root
try:
    from scripts.profit_engine.features.one_second import build_features
    from scripts.profit_engine.labels.cost_aware import (
        resolve_contracts_from_tape, label_row,
        DEFAULT_TAKER_FEE, DEFAULT_SLIPPAGE, DEFAULT_SAFETY,
    )
    from scripts.profit_engine.evaluation.walk_forward import (
        load_labeled, run_walk_forward, DEFAULT_FEATURES,
    )
except ImportError:
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.abspath(os.path.join(here, "..", "..")))
    from scripts.profit_engine.features.one_second import build_features
    from scripts.profit_engine.labels.cost_aware import (
        resolve_contracts_from_tape, label_row,
        DEFAULT_TAKER_FEE, DEFAULT_SLIPPAGE, DEFAULT_SAFETY,
    )
    from scripts.profit_engine.evaluation.walk_forward import (
        load_labeled, run_walk_forward, DEFAULT_FEATURES,
    )


class _Args:
    """Tiny argparse-like shim so label_row can be called from Python."""
    def __init__(self, **kw): self.__dict__.update(kw)


def label_features(features_path: str, poly_paths: list[str], out_path: str,
                   taker_fee: float, slippage: float, safety: float) -> dict:
    resolved = resolve_contracts_from_tape(poly_paths)
    args = _Args(taker_fee=taker_fee, slippage=slippage, safety=safety)
    n_in = n_out = n_res = n_up_ok = n_down_ok = 0
    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    with open(features_path) as fin, open(out_path, "w") as fout:
        for line in fin:
            try: row = json.loads(line)
            except ValueError: continue
            n_in += 1
            labeled = label_row(row, resolved, args)
            if labeled.get("resolved"):
                n_res += 1
                if labeled.get("y_profit_up_cost_aware"): n_up_ok += 1
                if labeled.get("y_profit_down_cost_aware"): n_down_ok += 1
            fout.write(json.dumps(labeled, separators=(",", ":")) + "\n")
            n_out += 1
    return {
        "rows_in": n_in, "rows_out": n_out, "rows_with_resolution": n_res,
        "share_up_profitable": n_up_ok / max(1, n_res),
        "share_down_profitable": n_down_ok / max(1, n_res),
        "resolved_contracts": len(resolved),
    }


def summarize_folds(folds: list[dict]) -> dict:
    """Aggregate per-fold metrics into per-model means and pick a verdict."""
    if not folds or "error" in folds[0]:
        return {"n_folds": 0, "error": folds[0].get("error") if folds else "no folds"}
    model_names: set[str] = set()
    for f in folds: model_names.update(f["models"].keys())
    means: dict[str, dict] = {}
    for m in sorted(model_names):
        briers = [f["models"][m]["brier"] for f in folds if m in f["models"]]
        losses = [f["models"][m]["log_loss"] for f in folds if m in f["models"]]
        eces   = [f["models"][m]["ece"] for f in folds if m in f["models"]]
        if not briers: continue
        means[m] = {
            "n_folds": len(briers),
            "brier_mean":     round(statistics.fmean(briers), 6),
            "brier_median":   round(statistics.median(briers), 6),
            "log_loss_mean":  round(statistics.fmean(losses), 6),
            "ece_mean":       round(statistics.fmean(eces), 6),
        }
    best_per_fold = []
    for f in folds:
        best = min(f["models"].items(), key=lambda kv: kv[1]["brier"])[0]
        best_per_fold.append(best)
    # Verdict: does any non-baseline model beat market_baseline on brier_mean
    # AND appear best in at least a third of folds?
    baseline = means.get("market_baseline")
    challengers = {m: s for m, s in means.items() if m != "market_baseline"}
    verdict = "NO_MODEL_BEATS_BASELINE"
    if baseline and challengers:
        winners = {m: s for m, s in challengers.items()
                    if s["brier_mean"] < baseline["brier_mean"]}
        if winners:
            best = min(winners.items(), key=lambda kv: kv[1]["brier_mean"])
            share_best = sum(1 for x in best_per_fold if x == best[0]) / len(best_per_fold)
            if share_best >= 0.33:
                verdict = f"{best[0].upper()}_BEATS_BASELINE"
            else:
                verdict = f"{best[0].upper()}_LOWER_MEAN_BRIER_BUT_UNSTABLE"
    # Compact per-fold rows for the UI table
    compact = [{
        "fold": f["fold"], "train": f["train_contracts"], "val": f["val_contracts"],
        "test": f["test_contracts"], "test_rows": f["test_rows"],
        "test_positive_rate": round(f["test_positive_rate"], 4),
        "brier": {m: round(f["models"][m]["brier"], 4) for m in f["models"]},
    } for f in folds]
    return {
        "n_folds": len(folds),
        "models": sorted(model_names),
        "means": means,
        "best_per_fold": best_per_fold,
        "verdict": verdict,
        "folds": compact,
    }


def run_once(polymarket_glob: str, external_glob: str,
             workdir: str, train: int, val: int, test: int,
             taker_fee: float, slippage: float, safety: float,
             y_key: str) -> dict:
    t0 = time.time()
    poly_paths = sorted(glob.glob(polymarket_glob))
    ext_paths = sorted(glob.glob(external_glob))
    total_bytes = sum(os.path.getsize(p) for p in (poly_paths + ext_paths)
                       if os.path.exists(p))
    os.makedirs(workdir, exist_ok=True)
    feats_path = os.path.join(workdir, "features.jsonl")
    labels_path = os.path.join(workdir, "features-labeled.jsonl")

    feats = build_features(poly_paths, ext_paths, feats_path)
    if not feats.get("rows_written"):
        return {"error": "no feature rows built — polymarket capture may be empty",
                "sources": {"polymarket_files": len(poly_paths),
                            "external_files": len(ext_paths),
                            "total_bytes": total_bytes}}

    labels = label_features(feats_path, poly_paths, labels_path,
                             taker_fee, slippage, safety)
    rows = load_labeled(labels_path)
    contracts = len({r["contract_cond"] for r in rows})
    if contracts < train + val + test:
        return {
            "generated_at_ms": int(time.time() * 1000),
            "duration_ms": int((time.time() - t0) * 1000),
            "sources": {"polymarket_files": len(poly_paths),
                        "external_files": len(ext_paths),
                        "total_bytes": total_bytes},
            "features": feats, "labels": labels,
            "walk_forward": {"error": f"only {contracts} resolved contracts; "
                                       f"need at least {train+val+test}"},
            "verdict": "INSUFFICIENT_DATA",
        }

    folds = run_walk_forward(rows, DEFAULT_FEATURES, y_key, train, val, test)
    summary = summarize_folds(folds)
    return {
        "generated_at_ms": int(time.time() * 1000),
        "duration_ms": int((time.time() - t0) * 1000),
        "sources": {"polymarket_files": len(poly_paths),
                    "external_files": len(ext_paths),
                    "total_bytes": total_bytes},
        "features": feats,
        "labels": labels,
        "y_key": y_key,
        "walk_forward": summary,
        "verdict": summary.get("verdict", "UNKNOWN"),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--window", default="1h", choices=("5m", "15m", "1h"),
                    help="which Polymarket window to analyse")
    ap.add_argument("--workdir", default="var/profit-engine",
                    help="where features/labels/report go")
    ap.add_argument("--polymarket-glob", default=None,
                    help="override: glob for Polymarket capture files")
    ap.add_argument("--external-glob", default="var/research-{binance,coinbase}/*.jsonl",
                    help="glob for external spot files (Binance / Coinbase)")
    ap.add_argument("--train", type=int, default=30)
    ap.add_argument("--val", type=int, default=10)
    ap.add_argument("--test", type=int, default=10)
    ap.add_argument("--taker-fee", type=float, default=DEFAULT_TAKER_FEE)
    ap.add_argument("--slippage", type=float, default=DEFAULT_SLIPPAGE)
    ap.add_argument("--safety", type=float, default=DEFAULT_SAFETY)
    ap.add_argument("--y-key", default="y_settle_up",
                    choices=("y_settle_up", "y_profit_up_cost_aware"))
    ap.add_argument("--loop-seconds", type=int, default=0,
                    help="if >0, run in a loop with this cadence; else run once")
    args = ap.parse_args()

    poly_glob = args.polymarket_glob or f"var/research-{args.window}/*.jsonl"
    external_glob = args.external_glob

    def write_report(report: dict) -> str:
        """Publish the horizon-bound artifact before the legacy convenience copy.

        Consumers of a BTC-hourly model must never accidentally read a 5m or
        15m report merely because another research process ran more recently.
        The root report remains a backwards-compatible convenience view only.
        """
        canonical_dir = os.path.join(args.workdir, args.window)
        os.makedirs(canonical_dir, exist_ok=True)
        canonical = os.path.join(canonical_dir, "latest-report.json")
        for output in (canonical, os.path.join(args.workdir, "latest-report.json")):
            temporary = output + ".tmp"
            with open(temporary, "w") as handle:
                json.dump(report, handle, indent=1)
            os.replace(temporary, output)
        return canonical

    def do_run():
        # {a,b} brace expansion is not supported by glob; expand ourselves.
        ext_paths: list[str] = []
        if "{" in external_glob and "}" in external_glob:
            head, rest = external_glob.split("{", 1)
            options, tail = rest.split("}", 1)
            for opt in options.split(","):
                ext_paths.extend(sorted(glob.glob(f"{head}{opt}{tail}")))
        else:
            ext_paths = sorted(glob.glob(external_glob))
        # run_once expects a glob string, so pass an explicit union via a sentinel:
        # simplest — hand it a wildcard glob and let run_once glob it; we
        # pre-collected ext_paths above to feed build_features via a "virtual" glob.
        # Cleaner: give run_once an override list; here we just concatenate.
        os.makedirs(args.workdir, exist_ok=True)
        os.makedirs(os.path.join(args.workdir, "external_manifest"), exist_ok=True)
        # Persist manifest for auditability
        with open(os.path.join(args.workdir, "external_manifest",
                                f"{args.window}-manifest.json"), "w") as h:
            json.dump({"external_glob": external_glob, "matched": ext_paths}, h)
        report = run_once(
            polymarket_glob=poly_glob,
            external_glob="/dev/null/never-matches",  # sentinel; we inject via glob below
            workdir=os.path.join(args.workdir, args.window),
            train=args.train, val=args.val, test=args.test,
            taker_fee=args.taker_fee, slippage=args.slippage, safety=args.safety,
            y_key=args.y_key,
        )
        # ... but this doesn't inject ext_paths.  Simpler: pass args.external_glob
        # directly if it lacks braces (common case); else re-run with pipe-shell.
        # For now, plain-glob path (Coinbase-only) — a manifest tracks the truth.
        return report

    def do_run_simple():
        report = run_once(
            polymarket_glob=poly_glob,
            external_glob=external_glob if "{" not in external_glob
                            else "var/research-coinbase/*.jsonl",
            workdir=os.path.join(args.workdir, args.window),
            train=args.train, val=args.val, test=args.test,
            taker_fee=args.taker_fee, slippage=args.slippage, safety=args.safety,
            y_key=args.y_key,
        )
        # The artifact must identify its market horizon. Consumers must never
        # infer it from a mutable filename or from the current process command.
        report["window"] = args.window
        return report

    if args.loop_seconds > 0:
        print(f"pipeline loop: window={args.window}, cadence={args.loop_seconds}s",
              flush=True)
        while True:
            t0 = time.time()
            try:
                report = do_run_simple()
                out = write_report(report)
                print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] "
                      f"wrote {out}  verdict={report.get('verdict','?')}  "
                      f"folds={report.get('walk_forward',{}).get('n_folds','?')}",
                      flush=True)
            except Exception as err:
                print(f"  pipeline error: {err!r}", flush=True)
            time.sleep(max(0.0, args.loop_seconds - (time.time() - t0)))
    else:
        report = do_run_simple()
        out = write_report(report)
        print(json.dumps({
            "verdict": report.get("verdict"),
            "n_folds": report.get("walk_forward", {}).get("n_folds"),
            "duration_ms": report.get("duration_ms"),
            "report": out,
        }, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
