# Connect ChatGPT Pro to Poly Advisor

ChatGPT **Pro can use your paper data** — but only over **remote HTTPS MCP**,
not local stdio (what Claude Desktop / Cursor use).

Your Pro plan is a good fit for heavy analysis. Limits still exist on every
product; Pro usually gives more headroom than free Cursor quotas.

## Important OpenAI facts (2026)

| Capability | ChatGPT Pro |
|---|---|
| Custom MCP (Developer Mode) | Yes (web) |
| Local `node dist/index.js` stdio | **No** |
| Remote HTTPS MCP URL | **Yes** |
| Tool permissions | Often **read/fetch** only on Pro |
| Write tools (`save_suggestion`, etc.) | May be blocked on Pro; use Claude/Cursor to persist, or paste results |

Our advisor is **paper research only** — analysis tools are the main value for Pro.

## One-time setup

### 1. Start paper stack + snapshot (already usual)

Terminal + `btc-window-paper` + optional snapshot daemon.

### 2. Start HTTP MCP on the laptop

```bash
./scripts/run-poly-advisor-http.sh
```

Listens on `http://127.0.0.1:8787/mcp`  
Token is printed and saved to `var/poly-advisor/http.token`.

### 3. Expose HTTPS tunnel (ChatGPT cannot reach localhost)

Pick one:

```bash
# Cloudflare (recommended)
cloudflared tunnel --url http://127.0.0.1:8787

# or ngrok
ngrok http 8787
```

Copy the `https://….trycloudflare.com` (or ngrok) URL → use `https://YOUR_HOST/mcp`.

### 4. Add connector in ChatGPT (web)

1. Open ChatGPT → **Settings → Apps → Advanced → Developer mode** (enable)
2. **Create app / Add MCP server**
3. Server URL: `https://YOUR_TUNNEL_HOST/mcp`
4. Auth: Bearer token = contents of `var/poly-advisor/http.token`
5. Enable the read tools you want (`daily_briefing`, `get_feature_snapshot`, …)

### 5. Ask ChatGPT

> Read the poly advisor features and give a daily briefing. Prefer expectancy over win rate. Propose DISABLE/CAP_TIER actions only from the allowed menu. Do not suggest live trading.

## Security

- Keep the tunnel private; rotate `POLY_ADVISOR_HTTP_TOKEN` if the URL leaks.
- Do not put wallet keys or live trading credentials in this server (there are none).
- Prefer Cloudflare quick tunnel only while you are analyzing; stop when done.

## If write tools are greyed out on Pro

That’s expected on some Pro accounts. Still do full analysis in ChatGPT, then:
- paste the suggestion JSON into Cursor/Claude and call `save_suggestion`, or
- save manually under `var/poly-advisor/suggestions/`.

## vs Claude / Cursor

| Host | How it connects |
|---|---|
| Cursor / Claude Desktop | Local stdio (`run-poly-advisor-mcp.sh`) |
| ChatGPT Pro | HTTPS + Bearer (`run-poly-advisor-http.sh` + tunnel) |

Same tools, different pipe.
