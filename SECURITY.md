# Security Policy

This is a financial trading platform. Security and safety are first-order
concerns, and the codebase is deliberately built to hold no live authority: no
credential, private key, wallet action, signer, authenticated transport, or
order-submission path exists in the repository (see `docs/` and `AGENTS.md`).

## Reporting a vulnerability

Please report suspected vulnerabilities privately. Do **not** open a public
issue for a security problem.

- Preferred: open a [GitHub private security advisory](https://github.com/gopalakrishnachennu/poly-trading/security/advisories/new).
- Include: affected component/crate, version or commit, reproduction steps, and
  the impact you observed.

You can expect an acknowledgement within a few business days. Please allow
reasonable time for a fix before any public disclosure.

## Scope of particular interest

- Anything that could introduce a signing, submission, credential, or
  order-execution path where the design says none exists.
- Any way to create new market exposure while authoritative state is stale,
  unknown, or unreconciled.
- Determinism/recovery breaks: a path where corrupt durable state is silently
  skipped instead of halting recovery.
- Fixed-point/accounting errors, or any use of binary floating point for money,
  price, quantity, fees, or capital floors.
- Secrets, keys, wallet material, or database dumps committed to Git or exposed
  in logs.

## Handling of secrets

Never commit real credentials, keys, or wallet material. `.env*` and known key
material patterns are Git-ignored; `deploy/.env.example` contains placeholders
only. Report any secret found in history so it can be rotated and purged.

## Supply chain

Dependencies are audited in CI with `cargo deny` (licenses, advisories, sources)
and kept current via Dependabot. The pinned Rust toolchain and pinned TLC jar
(by SHA-256) are part of the trust boundary — changes to those are security
sensitive.
