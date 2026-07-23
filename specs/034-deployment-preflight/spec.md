# Phase 2.21 Specification: Offline Deployment Preflight and Operator Ceremony

## Objective

Verify one current, unrevoked Phase 2.20 readiness subject against immutable
regional deployment configuration, least-privilege limits, a rollback package,
and distinct release/risk/operations operator decisions, then emit a canonical
non-deploying preflight report.

## Scope

- Phase 2.20 exposes a digest-bound current-readiness snapshot only while its
  positive dossier remains current and no revocation exists.
- One single writer owns one immutable deployment package.
- The package binds the exact fleet dossier, release, artifacts, rollback
  subject, every planned region, least-privilege policy, rollback artifacts,
  creation time and expiry.
- Regional configurations are canonical, unique and exactly equal to the
  completed Phase 2.20 region set.
- Least privilege forbids withdrawal, arbitrary transfer, contract upgrade,
  public administration and embedded credential material.
- Release, risk and operations approvals bind the exact package, are fresh,
  role-distinct and come from three distinct opaque operator identities.
- Finalization requires a fresh current Phase 2.20 readiness snapshot so a
  revoked or superseded dossier cannot remain eligible.
- Commands are journaled and checkpointed. Reports are canonical, create-new,
  checksummed, and grant no deployment, credential, signing or trading authority.

## Exclusions

- Cloud, cluster, DNS, load-balancer or routing control
- Credentials, secret material, private keys, signatures or key ceremonies
- Authenticated transport, RPC, wallet, relayer or signer processes
- Artifact upload, deployment, rollback execution, order or transaction submission

## Acceptance criteria

- Stale, revoked, non-ready, authority-bearing or subject-mismatched fleet
  evidence fails closed or produces an attributable denial.
- Regional identity, image, configuration, infrastructure, network,
  observability and failover digests are exact and immutable.
- Missing, duplicate or extra regions cannot pass preflight.
- Privilege escalation, credential material, public administration, arbitrary
  transfer, withdrawal or contract-upgrade capability cannot pass.
- Rollback evidence binds the exact release/artifact/rollback subject and is
  fresh at package registration.
- Missing, expired, rejected, same-operator or wrong-subject ceremony decisions
  cannot pass.
- Finalization rechecks a fresh current Phase 2.20 readiness binding.
- Exact command replay is idempotent; conflicting identity reuse and history
  equivocation halt absorbingly.
- Replay/checkpoint recovery reproduces complete state. Report corruption,
  replacement and noncanonical encoding are rejected.
- Tests cover positive preflight, region and privilege failures, rollback
  substitution, role/operator gates, expiry, post-package fleet invalidation,
  deterministic replay, corruption and durable sync failure.
- A bounded TLA+ model proves package, region, privilege, rollback, ceremony,
  current-fleet, no-authority and absorbing-halt gates.
