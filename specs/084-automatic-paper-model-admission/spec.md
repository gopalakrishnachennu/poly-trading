# Phase 5.18 Specification: Automatic Paper-Model Admission

## Objective

Automatically register a model for paper-only evaluation after it passes every
frozen development and unseen-test gate, without granting capital, placement,
signing, submission or live authority.

## Scope

- Require explicit policy opt-in for automatic paper admission.
- Reverify the cycle, report and model-file checksums.
- Require `PAPER_CANDIDATE_ONLY`, `final_test_evaluated=true` and complete final
  metrics from the accepted trainer.
- Publish an immutable per-cycle admission and one checksum-bound paper registry
  pointer.
- Publish a negative admission record when no model passes.

## Exclusions

- Automatic live promotion, external orders, credentials, wallet access,
  bypassing Paper Week risk/accounting, or treating admission as realized P&L.

## Acceptance criteria

- Rejected, missing, stale, malformed or checksum-mismatched models are never
  registered.
- A passing model is registered only for paper simulation and all external
  authority flags remain false.
- Repeated identical cycles produce the same admission.
- The terminal distinguishes `PAPER_AUTO_ADMITTED` from live authorization.
