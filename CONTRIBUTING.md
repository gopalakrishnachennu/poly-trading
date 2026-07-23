# Contributing

This is a proprietary, safety-first prediction-market trading platform. Changes
must preserve every financial, recovery, reconciliation, and authority boundary.

**Read [AGENTS.md](AGENTS.md) first** — it defines the mission, the required
reading order, the non-negotiable rules, and the definition of done. This file
covers the mechanics.

## Prerequisites

- Rust (pinned by `rust-toolchain.toml`, currently 1.88)
- Node.js `>=22.13.0` (for the operator terminal under `terminal/`)
- `cargo-deny` for the dependency audit: `cargo install cargo-deny --locked`
- Java 21 + the pinned TLC jar for the formal models (see `make tla`)

## The gates

Everything CI enforces is reproducible locally. The single command:

```bash
make verify      # fmt + clippy + workspace tests + cargo-deny + terminal lint/test
```

Individual targets: `make fmt`, `make clippy`, `make test`, `make deny`,
`make terminal`, `make tla`, `make ci` (verify + tla). Run `make help` for the
full list.

## Workflow

1. Work from a specification under `specs/` with explicit acceptance criteria.
2. Keep scope within the current milestone; don't add unrelated abstraction.
3. Branch off `main`; do not commit directly to `main`.
4. Add or update tests with every behavior change — include failure behavior,
   not only happy paths.
5. Record architecture changes as ADRs under `docs/adr/`; never silently reverse
   an accepted ADR.
6. Run `make verify` (and `make tla` if a formal model changed) before pushing.
7. Update `docs/CURRENT_STATE.md` / `docs/STATUS.md` only when milestone status
   materially changes.
8. Open a PR using the template; fill in the invariant and gate checklists.

## Non-negotiables (summary — see AGENTS.md for the full list)

- Never represent money, price, quantity, fees, or capital floors with binary
  floating point.
- Strategy code may never sign or submit an order directly.
- Matched inventory is not confirmed or spendable inventory.
- No new exposure while authoritative state is stale, unknown, or unreconciled.
- Journal every recovery-relevant transition; corrupt durable state must halt
  recovery, never be silently skipped.
- No secrets, keys, wallet material, or database dumps in Git.
- Unsafe Rust is forbidden unless an accepted ADR isolates and justifies it.

## Commit messages

Write imperative, present-tense summaries that explain the *why*. Reference the
spec or milestone where relevant.
