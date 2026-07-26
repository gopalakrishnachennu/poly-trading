#!/usr/bin/env python3
"""Run an immutable, non-authorizing rolling BTC-hourly research cycle."""
from __future__ import annotations

import argparse
import fcntl
import glob
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator


class InvalidRollingCycle(ValueError):
    """The rolling cycle cannot safely proceed."""


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def resolve(root: Path, value: str) -> Path:
    candidate = Path(value)
    return (candidate if candidate.is_absolute() else root / candidate).resolve()


def positive_integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise InvalidRollingCycle(f"{field} must be a positive integer")
    return value


def read_config(path: Path, project_root: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "book_source_glob",
        "reference_source_glob", "source_blob_root", "cycle_root", "lock_path",
        "historical_tape_root", "historical_auxiliary_root",
        "research_policy_path", "classifier_policy_path",
        "net_value_policy_path", "maximum_source_files_per_group",
        "maximum_source_bytes_per_file", "maximum_source_rows_per_file",
        "maximum_book_levels", "retrain_interval_seconds",
        "automatic_paper_admission",
    }
    if set(raw) != expected or raw["schema_version"] != 1 or raw["asset"] != "BTC":
        raise InvalidRollingCycle("rolling research config schema invalid")
    for field in (
        "maximum_source_files_per_group", "maximum_source_bytes_per_file",
        "maximum_source_rows_per_file", "maximum_book_levels",
        "retrain_interval_seconds",
    ):
        positive_integer(raw[field], field)
    for field in expected - {
        "schema_version", "asset", "maximum_source_files_per_group",
        "maximum_source_bytes_per_file", "maximum_source_rows_per_file",
        "maximum_book_levels", "retrain_interval_seconds",
        "automatic_paper_admission",
    }:
        if not isinstance(raw[field], str) or not raw[field]:
            raise InvalidRollingCycle(f"{field} must be a non-empty string")
    if raw["automatic_paper_admission"] is not True:
        raise InvalidRollingCycle("automatic paper admission requires explicit true")
    raw["_project_root"] = project_root
    return raw


@contextmanager
def exclusive_lock(path: Path) -> Iterator[None]:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise InvalidRollingCycle("another BTC-hourly research cycle is running") from error
        yield
    finally:
        os.close(descriptor)


def matching_sources(project_root: Path, pattern: str, maximum: int) -> list[Path]:
    absolute_pattern = str(resolve(project_root, pattern))
    paths = [Path(value).resolve() for value in sorted(glob.glob(absolute_pattern))]
    if not paths:
        raise InvalidRollingCycle(f"source glob matched no files: {pattern}")
    if len(paths) > maximum:
        raise InvalidRollingCycle(f"source glob exceeds file-count bound: {pattern}")
    if len(set(paths)) != len(paths) or any(path.is_symlink() or not path.is_file() for path in paths):
        raise InvalidRollingCycle("source set contains duplicate, missing or unsafe file")
    return paths


def freeze_jsonl_prefix(
    source: Path,
    blob_root: Path,
    maximum_bytes: int,
    maximum_rows: int,
) -> dict[str, Any]:
    before = source.stat()
    if before.st_size <= 0 or before.st_size > maximum_bytes:
        raise InvalidRollingCycle(f"source byte bound violated: {source}")
    blob_root.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        prefix=".prefix-", suffix=".jsonl", dir=blob_root, delete=False,
    ) as temporary:
        temporary_path = Path(temporary.name)
        remaining = before.st_size
        rows = 0
        digest = hashlib.sha256()
        trailing = b""
        with source.open("rb") as handle:
            while remaining:
                chunk = handle.read(min(1 << 20, remaining))
                if not chunk:
                    raise InvalidRollingCycle(f"source truncated during freeze: {source}")
                remaining -= len(chunk)
                data = trailing + chunk
                if remaining:
                    split = data.rfind(b"\n")
                    if split < 0:
                        trailing = data
                        if len(trailing) > 16 << 20:
                            raise InvalidRollingCycle("JSONL record exceeds bounded prefix buffer")
                        continue
                    complete, trailing = data[:split + 1], data[split + 1:]
                else:
                    if data.endswith(b"\n"):
                        complete, trailing = data, b""
                    else:
                        split = data.rfind(b"\n")
                        complete = b"" if split < 0 else data[:split + 1]
                        trailing = data if split < 0 else data[split + 1:]
                if complete:
                    temporary.write(complete)
                    digest.update(complete)
                    rows += complete.count(b"\n")
                    if rows > maximum_rows:
                        raise InvalidRollingCycle(f"source row bound violated: {source}")
        temporary.flush()
        os.fsync(temporary.fileno())
    after = source.stat()
    if before.st_ino != after.st_ino or after.st_size < before.st_size:
        temporary_path.unlink(missing_ok=True)
        raise InvalidRollingCycle(f"source replaced or truncated during freeze: {source}")
    if rows == 0:
        temporary_path.unlink(missing_ok=True)
        raise InvalidRollingCycle(f"source has no complete records: {source}")
    checksum = digest.hexdigest()
    destination = blob_root / f"{checksum}.jsonl"
    frozen_bytes = temporary_path.stat().st_size
    if destination.exists():
        if destination.is_symlink() or sha256_file(destination) != checksum:
            temporary_path.unlink(missing_ok=True)
            raise InvalidRollingCycle("content-addressed source blob conflict")
        temporary_path.unlink()
    else:
        os.replace(temporary_path, destination)
        directory_descriptor = os.open(blob_root, os.O_RDONLY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    return {
        "source_path": str(source),
        "source_size_at_freeze": before.st_size,
        "excluded_incomplete_tail_bytes": len(trailing),
        "path": str(destination),
        "sha256": checksum,
        "maximum_bytes": frozen_bytes,
        "maximum_rows": rows,
    }


def run_step(
    project_root: Path,
    staging: Path,
    name: str,
    arguments: list[str],
) -> None:
    completed = subprocess.run(
        [sys.executable, *arguments],
        cwd=project_root,
        text=True,
        capture_output=True,
        check=False,
    )
    log = {
        "schema_version": 1,
        "step": name,
        "command": arguments,
        "returncode": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    (staging / f"{name}.json").write_bytes(canonical(log) + b"\n")
    if completed.returncode != 0:
        raise InvalidRollingCycle(f"{name} failed; see preserved step log")


def atomic_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with temporary.open("xb") as output:
        output.write(canonical(value) + b"\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def build_paper_admission(
    revision: str,
    models: list[tuple[str, dict[str, Any], Path]],
) -> dict[str, Any]:
    candidates: list[dict[str, Any]] = []
    for model_kind, model_report, model_root in models:
        if model_report["verdict"] != "PAPER_CANDIDATE_ONLY":
            continue
        model_path = model_root / "model.txt"
        if (
            model_report.get("final_test_evaluated") is not True
            or not isinstance(model_report.get("final_metrics"), dict)
            or not model_path.is_file()
            or sha256_file(model_path) != model_report.get("model_sha256")
        ):
            raise InvalidRollingCycle("paper candidate model evidence is incomplete")
        candidates.append({
            "model_kind": model_kind,
            "model_sha256": model_report["model_sha256"],
            "report_sha256": sha256_file(model_root / "report.json"),
            "final_metrics": model_report["final_metrics"],
        })
    paper_candidate = bool(candidates)
    return {
        "schema_version": 1,
        "paper_only": True,
        "automatic": True,
        "cycle_revision": revision,
        "status": "AUTO_ADMITTED_PAPER_ONLY" if paper_candidate else "NO_ELIGIBLE_MODEL",
        "paper_simulation_eligible": paper_candidate,
        "models": candidates,
        "capital_authority": False,
        "placement_authority": False,
        "signing_authority": False,
        "submission_authority": False,
        "live_trading_authority": False,
    }


def publish_paper_registry(
    cycles: Path,
    cycle_root: Path,
    report: dict[str, Any],
) -> None:
    admission_path = cycle_root / "paper-admission.json"
    if not admission_path.is_file():
        raise InvalidRollingCycle("completed cycle is missing paper admission evidence")
    admission = json.loads(admission_path.read_text())
    admission_sha256 = sha256_file(admission_path)
    if admission_sha256 != report.get("paper_admission_sha256"):
        raise InvalidRollingCycle("paper admission checksum does not match cycle report")
    expected_status = (
        "AUTO_ADMITTED_PAPER_ONLY"
        if report.get("decision") == "PAPER_AUTO_ADMITTED"
        else "NO_ELIGIBLE_MODEL"
    )
    if admission.get("status") != expected_status:
        raise InvalidRollingCycle("paper admission status does not match cycle decision")
    for authority in (
        "capital_authority",
        "placement_authority",
        "signing_authority",
        "submission_authority",
        "live_trading_authority",
    ):
        if admission.get(authority) is not False:
            raise InvalidRollingCycle(
                f"paper admission unexpectedly grants {authority}"
            )
    atomic_json(cycles.parent / "paper-model-registry" / "latest.json", {
        "schema_version": 1,
        "paper_only": True,
        "cycle_revision": report["revision"],
        "status": admission["status"],
        "paper_simulation_eligible": admission["paper_simulation_eligible"],
        "models": admission["models"],
        "admission_path": str(admission_path),
        "admission_sha256": admission_sha256,
    })


def cycle_once(config_path: Path, config: dict[str, Any]) -> dict[str, Any]:
    project_root: Path = config["_project_root"]
    maximum_files = config["maximum_source_files_per_group"]
    book_sources = matching_sources(project_root, config["book_source_glob"], maximum_files)
    reference_sources = matching_sources(
        project_root, config["reference_source_glob"], maximum_files,
    )
    blob_root = resolve(project_root, config["source_blob_root"])
    frozen_books = [
        freeze_jsonl_prefix(
            path, blob_root, config["maximum_source_bytes_per_file"],
            config["maximum_source_rows_per_file"],
        )
        for path in book_sources
    ]
    frozen_references = [
        freeze_jsonl_prefix(
            path, blob_root, config["maximum_source_bytes_per_file"],
            config["maximum_source_rows_per_file"],
        )
        for path in reference_sources
    ]
    cycles = resolve(project_root, config["cycle_root"])
    cycles.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".staging-", dir=cycles))
    try:
        local_policy = {
            "schema_version": 1,
            "asset": "BTC",
            "window_ms": 3_600_000,
            "maximum_book_levels": config["maximum_book_levels"],
            "source_time_semantics": "LOCAL_GATEWAY_POLL_OBSERVATION",
            "reference_source": "binance",
            "book_sources": [
                {key: row[key] for key in ("path", "sha256", "maximum_bytes", "maximum_rows")}
                for row in frozen_books
            ],
            "reference_sources": [
                {key: row[key] for key in ("path", "sha256", "maximum_bytes", "maximum_rows")}
                for row in frozen_references
            ],
        }
        local_policy_path = staging / "local-normalization-policy.json"
        local_policy_path.write_bytes(canonical(local_policy) + b"\n")
        run_step(
            project_root, staging, "01-normalize-local",
            [
                "scripts/normalize_local_btc_hourly_capture.py",
                "--policy", str(local_policy_path),
                "--out-root", str(staging / "local-evidence"),
            ],
        )
        local_manifest_path = staging / "local-evidence" / "manifest.json"
        local_manifest = json.loads(local_manifest_path.read_text())
        dependencies = {
            "schema_version": 1,
            "rolling_config_sha256": sha256_file(config_path),
            "local_completed_evidence": {
                "data_sha256": local_manifest["data_sha256"],
                "spot_sha256": local_manifest["spot_sha256"],
                "trades_sha256": local_manifest["trades_sha256"],
                "sessions": local_manifest["sessions"],
            },
            "historical_manifest_sha256": sha256_file(
                resolve(project_root, config["historical_tape_root"]) / "manifest.json"
            ),
            "historical_auxiliary_manifest_sha256": sha256_file(
                resolve(project_root, config["historical_auxiliary_root"]) / "manifest.json"
            ),
            "research_policy_sha256": sha256_file(
                resolve(project_root, config["research_policy_path"])
            ),
            "classifier_policy_sha256": sha256_file(
                resolve(project_root, config["classifier_policy_path"])
            ),
            "net_value_policy_sha256": sha256_file(
                resolve(project_root, config["net_value_policy_path"])
            ),
        }
        revision = hashlib.sha256(canonical(dependencies)).hexdigest()
        final_root = cycles / revision
        if final_root.exists():
            if final_root.is_symlink() or not (final_root / "cycle-report.json").is_file():
                raise InvalidRollingCycle("existing cycle revision is incomplete or unsafe")
            shutil.rmtree(staging)
            report = json.loads((final_root / "cycle-report.json").read_text())
            publish_paper_registry(cycles, final_root, report)
            report["idempotent_reuse"] = True
            return report
        final_root.mkdir(parents=False, exist_ok=False)
        for name in (
            "local-normalization-policy.json", "local-evidence",
            "01-normalize-local.json",
        ):
            os.replace(staging / name, final_root / name)
        staging.rmdir()
        run_step(
            project_root, final_root, "02-merge-evidence",
            [
                "scripts/merge_btc_hourly_research_evidence.py",
                "--historical", str(resolve(project_root, config["historical_tape_root"])),
                "--historical-auxiliary",
                str(resolve(project_root, config["historical_auxiliary_root"])),
                "--local", str(final_root / "local-evidence"),
                "--out-root", str(final_root / "combined-evidence"),
            ],
        )
        run_step(
            project_root, final_root, "03-build-dataset",
            [
                "scripts/build_btc_hourly_research.py",
                "--input", str(final_root / "combined-evidence" / "clob-l2.jsonl"),
                "--policy", str(resolve(project_root, config["research_policy_path"])),
                "--out-dir", str(final_root / "dataset"),
            ],
        )
        dataset_manifest = json.loads((final_root / "dataset" / "manifest.json").read_text())
        if dataset_manifest.get("eligibility") != "DATASET_ONLY_NOT_A_MODEL":
            raise InvalidRollingCycle("dataset is not eligible for model evaluation")
        run_step(
            project_root, final_root, "04-classifier",
            [
                "scripts/train_btc_hourly_path_model_v4.py",
                "--dataset", str(final_root / "dataset"),
                "--auxiliary", str(final_root / "combined-evidence"),
                "--policy", str(resolve(project_root, config["classifier_policy_path"])),
                "--out-dir", str(final_root / "classifier"),
            ],
        )
        run_step(
            project_root, final_root, "05-net-value",
            [
                "scripts/train_btc_hourly_path_value_v5.py",
                "--dataset", str(final_root / "dataset"),
                "--auxiliary", str(final_root / "combined-evidence"),
                "--policy", str(resolve(project_root, config["net_value_policy_path"])),
                "--out-dir", str(final_root / "net-value"),
            ],
        )
        classifier = json.loads((final_root / "classifier" / "report.json").read_text())
        net_value = json.loads((final_root / "net-value" / "report.json").read_text())
        allowed_classifier = {
            "REJECTED_NO_PATH_DEPENDENT_EDGE", "REJECTED_UNSEEN_TEST",
            "PAPER_CANDIDATE_ONLY",
        }
        allowed_value = {
            "REJECTED_NO_PATH_NET_VALUE_EDGE", "REJECTED_UNSEEN_TEST",
            "PAPER_CANDIDATE_ONLY",
        }
        if classifier.get("verdict") not in allowed_classifier:
            raise InvalidRollingCycle("classifier returned an unknown verdict")
        if net_value.get("verdict") not in allowed_value:
            raise InvalidRollingCycle("net-value model returned an unknown verdict")
        admission = build_paper_admission(revision, [
            ("path_classifier", classifier, final_root / "classifier"),
            ("path_net_value", net_value, final_root / "net-value"),
        ])
        paper_candidate = admission["paper_simulation_eligible"]
        (final_root / "paper-admission.json").write_bytes(canonical(admission) + b"\n")
        report = {
            "schema_version": 1,
            "non_authoritative": True,
            "asset": "BTC",
            "window": "1h",
            "revision": revision,
            "generated_at_ms": int(time.time() * 1000),
            "decision": "PAPER_AUTO_ADMITTED" if paper_candidate else "NO_TRADE",
            "automatic_activation": False,
            "paper_admission_status": admission["status"],
            "paper_admission_sha256": sha256_file(final_root / "paper-admission.json"),
            "local_completed_sessions": local_manifest["sessions"],
            "dataset_sessions": sum(
                dataset_manifest["folds"][name]["sessions"]
                for name in ("train", "validation", "test")
            ),
            "classifier_verdict": classifier["verdict"],
            "net_value_verdict": net_value["verdict"],
            "dependencies": dependencies,
            "frozen_book_sources": frozen_books,
            "frozen_reference_sources": frozen_references,
        }
        (final_root / "cycle-report.json").write_bytes(canonical(report) + b"\n")
        latest = {
            "schema_version": 1,
            "non_authoritative": True,
            "revision": revision,
            "cycle_path": str(final_root),
            "decision": report["decision"],
            "generated_at_ms": report["generated_at_ms"],
            "cycle_report_sha256": sha256_file(final_root / "cycle-report.json"),
        }
        atomic_json(cycles / "latest.json", latest)
        publish_paper_registry(cycles, final_root, report)
        return report
    except Exception:
        if staging.exists():
            failed = cycles / f"failed-{int(time.time() * 1000)}-{os.getpid()}"
            os.replace(staging, failed)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--config", type=Path,
        default=Path("config/btc-hourly-rolling-research.example.json"),
    )
    parser.add_argument("--loop", action="store_true")
    arguments = parser.parse_args()
    project_root = Path(__file__).resolve().parent.parent
    try:
        config_path = arguments.config.resolve()
        config = read_config(config_path, project_root)
        lock_path = resolve(project_root, config["lock_path"])
        if not arguments.loop:
            with exclusive_lock(lock_path):
                result = cycle_once(config_path, config)
            print(json.dumps({
                "revision": result["revision"],
                "decision": result["decision"],
                "local_completed_sessions": result["local_completed_sessions"],
                "dataset_sessions": result["dataset_sessions"],
                "classifier_verdict": result["classifier_verdict"],
                "net_value_verdict": result["net_value_verdict"],
                "idempotent_reuse": result.get("idempotent_reuse", False),
            }, sort_keys=True), flush=True)
            return 0
        with exclusive_lock(lock_path):
            while True:
                started = time.monotonic()
                result = cycle_once(config_path, config)
                print(json.dumps({
                    "revision": result["revision"],
                    "decision": result["decision"],
                    "local_completed_sessions": result["local_completed_sessions"],
                    "dataset_sessions": result["dataset_sessions"],
                    "classifier_verdict": result["classifier_verdict"],
                    "net_value_verdict": result["net_value_verdict"],
                    "idempotent_reuse": result.get("idempotent_reuse", False),
                }, sort_keys=True), flush=True)
                remaining = config["retrain_interval_seconds"] - (time.monotonic() - started)
                time.sleep(max(0.0, remaining))
    except (InvalidRollingCycle, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"rolling BTC-hourly research rejected: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("rolling BTC-hourly research stopped by operator", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
