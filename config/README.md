# Paper market policy

`paper-market-policy.example.json` is an example of the immutable policy that
must be supplied before a new paper campaign can create simulated pairs.

Copy it to a local, untracked file, review each per-asset value, then start the
gateway with its explicit path:

```text
POLY_PAPER_POLICY_PATH=config/paper-market-policy.json
```

The policy binds the permitted asset identifiers and all paper-pair economics:
fee, slippage reserve, minimum locked edge, maximum pair quantity, and campaign
duration. Amounts
are integer micros (`1_000_000` micros equals one US dollar / token payout).
The gateway checks its validity interval and journals the canonical BLAKE3
digest at campaign start. Editing a file later does not change a running
campaign; after restart, an unavailable or mismatched policy makes recovery
observation-only.

This is a paper-only control. It grants no wallet, signing, authenticated
transport, order, split, merge, or live-trading authority.

## Campaign preflight and explicit start

Launching the terminal does **not** start or resume a paper recorder. Before
the dashboard enables **START PAPER**, the gateway checks the runtime binding,
current paper policy, permitted BTC/ETH assets, local clock, no-active-campaign
state, and a create/sync/cleanup probe of the configured journal directory.

If the process restarts during a campaign, it recovers the journal as evidence
but suspends the campaign. An operator must inspect the preflight and start a
new campaign deliberately. There is no automatic resume based on a healthy
market feed.

## Terminal runtime configuration

`terminal-runtime.example.json` controls public feed endpoints, polling and
discovery budgets, projection freshness budgets, and browser display budgets.
Copy it to an untracked local file and set:

```text
POLY_TERMINAL_CONFIG_PATH=config/terminal-runtime.json
```

The gateway reads this document exactly once at startup, validates its expiry,
HTTPS endpoints, and bounded values, then exposes its ID and BLAKE3 digest in
the read-only **Settings** terminal workspace. It never watches, rewrites, or
hot-reloads the file. A changed file requires an explicit gateway restart.

Without a bound runtime configuration, the current process can retain
credentialless observation using compiled safe defaults, but is visibly marked
`LEGACY_DEFAULTS_OBSERVATION_ONLY` and cannot start a new paper campaign.
