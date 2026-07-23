<!-- Read AGENTS.md before opening a PR. Keep scope within the current milestone. -->

## Summary

<!-- What changed and why. -->

## Specification / milestone

<!-- Link the spec under specs/ or the milestone this advances. -->

## Financial & recovery invariants

<!-- Confirm none are weakened. See docs/FINANCIAL_INVARIANTS.md and AGENTS.md. -->

- [ ] No money/price/quantity/fee/capital value uses binary floating point.
- [ ] No new signing, submission, credential, wallet, or external-order path.
- [ ] New exposure is still forbidden on stale/unknown/unreconciled state.
- [ ] Every recovery-relevant state transition is journaled; corrupt state halts.

## Tests

- [ ] Behavior changes include tests, and failure behavior is covered (not only happy paths).

## Gates (must pass — see `make verify`)

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo deny --workspace check`
- [ ] Terminal: `cd terminal && npm run lint && npm test` (if the terminal changed)
- [ ] Affected TLA+ models still pass (if a formal model changed)

## ADRs / docs

- [ ] Architecture changes recorded as an ADR; no accepted ADR silently reversed.
- [ ] `docs/CURRENT_STATE.md` / `docs/STATUS.md` updated if milestone status materially changed.
