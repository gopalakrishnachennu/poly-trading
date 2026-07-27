# Poly Advisor MCP

Read-only research cockpit for Claude, ChatGPT, and Cursor over the BTC/ETH
window paper book (**Spec 094 / ADR 0098**).

This MCP **never** places, sizes, cancels, or signs orders. It reads live paper
state + journals, stores 1-second snapshots on this laptop, and persists
suggestions/patterns under `var/poly-advisor/`.

## Quick start

```bash
cd advisor
npm install
npm test
npm run build
```

### Snapshot daemon (1 Hz local capture)

```bash
# from repo root
python3 scripts/detach.py var/log/poly-advisor-snapshot.log \
  bash -lc 'cd advisor && node dist/snapshot_daemon.js'
# or
./scripts/run-poly-advisor-snapshot.sh
```

### Run MCP (stdio — Claude / Cursor)

```bash
./scripts/run-poly-advisor-mcp.sh
```

### Run MCP (HTTP — ChatGPT Pro)

```bash
./scripts/run-poly-advisor-http.sh
# then tunnel: cloudflared tunnel --url http://127.0.0.1:8787
```

See [CHATGPT.md](./CHATGPT.md) for Developer Mode steps.

## Connect Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "poly-advisor": {
      "command": "/Users/gopalakrishnachennu/Desktop/Devops-9/Poly-Trading/scripts/run-poly-advisor-mcp.sh",
      "env": {
        "POLY_ADVISOR_API_URL": "http://127.0.0.1:3000/api/btc-window-paper"
      }
    }
  }
}
```

Then ask Claude: *Read resource poly/features and run daily_briefing.*

## Connect ChatGPT

ChatGPT does not speak MCP stdio natively in all plans. Options:

1. **Cursor / Claude Code** — preferred MCP hosts for this server.
2. **Custom GPT Actions** — wrap selected tools behind a local HTTPS bridge later
   (not required for Claude).
3. Paste `get_feature_snapshot` JSON + the system prompt from
   `resource://poly/advisor-prompt` into ChatGPT manually.

## Env vars

| Var | Default |
|---|---|
| `POLY_ADVISOR_API_URL` | `http://127.0.0.1:3000/api/btc-window-paper` |
| `POLY_ADVISOR_ROOT` | `<repo>/var/poly-advisor` |
| `POLY_BTC_WINDOW_PAPER_DIR` | `<repo>/var/btc-window-paper` |
| `POLY_REPO_ROOT` | auto from package location |
| `POLY_ADVISOR_SNAPSHOT_MS` | `1000` |

## Tool modules

- **A Data** — status, capital, health, open book, scoreboard, diagnostics, tape, journal, snapshots
- **B Analytics** — kill/keep, death patterns, what-ifs, stake audit, edge vs size
- **C Patterns** — local algo library
- **D Suggestions** — save/accept/reject advisor runs
- **E Specs** — draft deltas, validate proposals, mark implemented
- **F Ops** — overview, halt diagnosis, lock, restart playbook
- **G Reports** — daily briefing, loss review, export
- **H Guardrails** — paper-only asserts, no live-order tools

## Storage

All under `var/poly-advisor/` (gitignored via `var/`).
