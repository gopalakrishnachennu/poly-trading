# Phase 2.18 Specification: Offline Promotion Governance and Release Sealing

## Objective

Aggregate independent Phase 2.17 campaign evidence, enforce deterministic
diversity and regression thresholds, record two distinct role-bound operator
decisions, and emit a checksummed non-deploying canary-eligibility record bound
to exact release artifacts and rollback criteria.

## Scope

- One single writer owns one release-candidate lifecycle.
- A submission contains bounded Phase 2.17 evidence bundles, a sealed regression
  baseline, sealed release artifacts, and sealed rollback criteria.
- Campaign eligibility, freshness, independence, diversity, and aggregate
  regression thresholds are recomputed from bundle contents.
- Evidence-set ordering is canonical; duplicate bundle, campaign, manifest, or
  schedule identities cannot inflate diversity.
- Risk and release decisions bind the exact candidate subject, have bounded
  validity, and must come from distinct opaque operator identities.
- Rejection is an attributable governance result. Identity reuse, digest
  substitution, time regression, or post-finalization mutation halts.
- Final output is a sealed canary-eligibility record. It grants no execution,
  credential, signing, deployment, promotion, wallet, RPC, or live authority.
- Commands and complete state are journaled and prefix-checkpointed. Canary
  records use create-new canonical checksummed files.

## Exclusions

- Credentials, private keys, signatures, identity-provider integration
- Authenticated transport, RPC, wallet or relayer clients
- Artifact building, registry upload, deployment, rollback execution
- Automatic promotion, canary launch, live orders, or production capital
- Replacing human accountability with an unverified operator identifier

## Acceptance criteria

- Invalid policy, evidence, baseline, artifact, rollback, or subject digests
  fail closed before installation.
- Eligibility requires the configured number of fresh, independently identified
  `PROMOTION_ELIGIBLE` campaigns and every absolute diversity threshold.
- Aggregate campaign, session, step, and fault counts meet both absolute floors
  and conservative basis-point retention against the sealed baseline.
- Risk and release approvals are current, bind the exact candidate digest, and
  use distinct nonzero operator identities. Either rejection denies eligibility.
- Artifact, source, dependency lock, toolchain, SBOM, configuration, rollback
  target, and rollback thresholds are immutable inputs to the candidate digest.
- Finalization emits attributable reasons and always sets all authority flags
  false, even when status is `CANARY_ELIGIBLE`.
- Exact duplicate commands are idempotent; conflicting command identities,
  decision identities, operator-role decisions, or final record identities halt.
- Journal replay and checkpoint recovery reproduce the complete governance
  digest. Evidence files reject replacement, corruption, noncanonical content,
  and invalid internal record digests.
- Tests cover success, stale/ineligible/duplicate campaigns, regression and
  diversity failure, same-operator denial, rejection/expiry, artifact
  substitution, restart recovery, corruption, and deterministic replay.
- A bounded TLA+ model proves evidence/regression/diversity/dual-control gates,
  no authority, single finalization, and absorbing halt.
