# Local production-shaped deployment (read-only)

This directory is a staging scaffold for the terminal projection. It runs the
Rust API, dashboard, Prometheus, and Grafana as separate containers. It is
deliberately **NO_TRADE**: there are no credentials, signers, wallets,
authenticated clients, or order routes in this deployment.

## Run locally

```sh
docker compose -f deploy/docker-compose.yml up --build
```

Then open <http://localhost:3000> (dashboard), <http://localhost:8088/healthz>
(API), and <http://localhost:3001> (Grafana). Stop with `docker compose -f
deploy/docker-compose.yml down`.

## Observability

The API exposes read-only Prometheus metrics at
<http://localhost:8088/metrics>:

- `poly_terminal_poll_success_total` / `poly_terminal_poll_failure_total`
  (counters)
- `poly_terminal_last_success_timestamp_ms` /
  `poly_terminal_last_failure_timestamp_ms` (gauges)

Prometheus (<http://localhost:9090>) scrapes the API every 15s and evaluates
[`prometheus/alerts.yml`](prometheus/alerts.yml):

- **TerminalProjectionPollFailures** — upstream polling has failed for ≥2m.
- **TerminalProjectionStale** — no successful refresh for >30s (NO_TRADE must
  remain enforced).

Grafana (<http://localhost:3001>, anonymous viewer) auto-provisions the
Prometheus datasource and the **Poly Terminal — Read-Only Projection** dashboard
(`uid: poly-terminal-projection`): projection freshness, gateway up/down,
cumulative failures, poll rate, and staleness against the 30s alert threshold.
The dashboard and datasource are provisioned read-only from
[`grafana/`](grafana); validate rules with
`promtool check config prometheus/prometheus.yml`.

## Optional durable-service fixture

To exercise local persistence, event infrastructure, and a free S3-compatible
archive, enable the `durable` profile. This starts disposable PostgreSQL,
Redpanda, ClickHouse, and MinIO volumes;
it does not connect the projection to them automatically and it cannot enable
orders or authenticated transport:

```sh
docker compose -f deploy/docker-compose.yml --profile durable up -d
docker compose -f deploy/docker-compose.yml --profile durable ps
```

MinIO is available at <http://localhost:9001> (console) and
`http://localhost:9002` (S3 API). Supply `MINIO_ROOT_USER` and
`MINIO_ROOT_PASSWORD` through an untracked `.env` file or the shell before
starting the profile. The committed [`.env.example`](.env.example) contains
placeholders only; never commit real credentials. The default placeholders are
intentionally not suitable for any shared or production environment.

The fixture uses PostgreSQL `trust` authentication and no network credentials,
because it is intended only for a local disposable host. It binds all ports to
`127.0.0.1`. Never expose this profile to a network or reuse its volumes for
production. Remove the fixture data with the explicit, local-only command:

```sh
docker compose -f deploy/docker-compose.yml --profile durable down -v
```

The service endpoints are `postgres:5432`, `redpanda:9092`, `clickhouse:8123`,
and `minio:9000` from the Compose network. The API remains `READ_ONLY` with
`POLY_TERMINAL_NO_TRADE=true` in every profile.

## Optional Vault Community fixture (development only)

Vault Community Edition can be started locally with the `security` profile:

```sh
docker compose -f deploy/docker-compose.yml --profile security up -d vault
docker compose -f deploy/docker-compose.yml --profile security logs vault
curl http://127.0.0.1:8200/v1/sys/health
```

This fixture runs Vault's in-memory **development server**. It has no persistent
volume and no preloaded root token; the dev server generates an ephemeral token
at startup. Treat that token and all values written to the fixture as disposable
test data. The API is not connected to Vault, and this profile does not provide
credential injection, signing, wallet access, or order capability. The listener
is bound to loopback only and TLS is intentionally disabled for local testing.

Do not use this mode for production, expose port `8200`, or reuse it as a
production secret store. Production requires a non-dev Vault server with
encrypted persistent storage, TLS, auto-unseal, policies, audit logging,
backups, and an independently managed recovery procedure. Stop the fixture with:

```sh
docker compose -f deploy/docker-compose.yml --profile security stop vault
```

The API also exposes the bounded, read-only Prometheus endpoint at
<http://localhost:8088/metrics>. Upstream URLs can be overridden for a
recorded-fixture or staging environment with `POLY_TERMINAL_GAMMA_URL`,
`POLY_TERMINAL_CLOB_BOOK_URL`, and `POLY_TERMINAL_BINANCE_URL`; unset values
retain the public defaults.

The API defaults to localhost when run directly. Compose explicitly sets
`POLY_TERMINAL_BIND=0.0.0.0` inside its isolated network and exposes only the
read-only HTTP endpoints. Replace no values with secrets: live connectivity is
not part of this compose file.

## Production hardening still required

Use a pinned image digest, a private network/ingress, non-root runtime users,
an externally managed secret manager, TLS termination, durable event storage,
backup/restore testing, and an independently reviewed change-control process
before any deployment outside a local or disposable staging environment. The
Vault fixture above is not that production secret manager.
