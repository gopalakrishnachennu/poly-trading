# Phase 5.2 Specification: Horizon-Bound Model Artifacts

## Objective

Ensure that the terminal's BTC one-hour model workspace displays only a
one-hour research artifact and cannot be overwritten by a concurrently running
short-window research process.

## Scope

- Publish each Profit Engine report under its explicit horizon directory.
- Retain the root report only as a backwards-compatible convenience copy.
- Make the terminal API require a valid `1h` artifact for the hourly model
  workspace.
- Display model evidence, resolved-hour count, and the current validation gate
  beside the real-time public-data telemetry.

## Exclusions

- Model training promotion, strategy approval, paper execution, live orders,
  wallet operations, credentials, or any claim of profitability.

## Acceptance criteria

- A 5m or 15m process cannot overwrite the terminal's selected BTC-hourly
  artifact.
- Missing or wrong-horizon artifacts fail closed with an explanatory status.
- The model workspace identifies the evidence horizon and validation gate.
- Python compilation, terminal lint, build, and tests pass.
