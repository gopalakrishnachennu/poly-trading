# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a proprietary,
never-published workspace, so entries track milestones and repository changes
rather than released package versions.

## [Unreleased]

### Added
- Repository standards: `LICENSE` (proprietary), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODEOWNERS`, PR template, `.editorconfig`, `CHANGELOG.md`.
- `Makefile` giving local/CI parity (`make verify`, `make ci`, `make tla`, …).
- Dependabot for cargo, npm (terminal), and GitHub Actions.
- CI: `cargo deny` job, terminal lint/build/test job, Rust build caching, and
  run-cancelling `concurrency`.
- `docs/STATUS.md`: scannable phase-gate and per-crate status summary.
- `docs/EDGE_ANALYSIS.md` and research tooling under `scripts/`:
  `analyze_paper_edge.py`, `backtest_fair_value.py` (with `--walk-forward` /
  `--sweep`), `capture_snapshots.py`, `run-continuous-capture.sh`,
  `capture_progress.py`.

### Changed
- Operator terminal migrated off Cloudflare/vinext/Wrangler to standard
  Next.js on Node; removed the unused D1 database, worker, and hosting scaffold.
- `cargo-deny` policy: `LicenseRef-Proprietary` + `publish = false` on all
  crates, `allow-wildcard-paths`, allow `CC0-1.0` / `CDLA-Permissive-2.0`,
  ignore `RUSTSEC-2024-0436` (transitive `paste`, no fix).

### Fixed
- CI: corrected the pinned `tla2tools.jar` SHA-256 (all TLA+ jobs had failed at
  download verification).
- Terminal: strict-mode type errors surfaced by `next build` (BigInt target,
  `Partial<Book>` narrowing, nullable `short()` argument).

### Notes
- Everything remains offline, read-only, and paper-only. No credential, signing,
  or order-submission capability was added.
