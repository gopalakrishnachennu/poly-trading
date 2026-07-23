# Poly Terminal

Read-only, paper-only operator terminal for the Poly Trading platform. It is a
standard [Next.js](https://nextjs.org) app (App Router) that runs on Node — no
Cloudflare, Wrangler, or external database dependency.

The UI renders exactly what the local Rust `terminal-projection` gateway
publishes on `http://127.0.0.1:8088`. It has no market, balance, risk, or
health fallback: any stale, partial, malformed, or unauthorized projection
clears the display and shows `GLOBAL NO_TRADE`.

## Prerequisites

- Node.js `>=22.13.0`
- For `dev:full`: a Rust toolchain (`cargo`) or a prebuilt
  `target/debug/terminal-projection` binary in the repository root.

## Quick start

Run the gateway and dashboard together (starts the Rust projection gateway if
one is not already healthy on `127.0.0.1:8088`, then the Next.js dev server):

```bash
npm install
npm run dev:full
```

Open `http://localhost:3000`.

To run only the dashboard against an already-running gateway:

```bash
npm run dev
```

## Scripts

- `npm run dev` — start the Next.js dev server (dashboard only)
- `npm run dev:full` — start the projection gateway (if needed) and the dashboard
- `npm run build` — production build
- `npm run start` — serve the production build (honors `PORT` / `HOSTNAME`)
- `npm test` — build, then verify the server-rendered shell fails closed
- `npm run lint` — ESLint

## Configuration

The dashboard reads a single environment variable:

- `NEXT_PUBLIC_TERMINAL_API_URL` — projection gateway base URL
  (default `http://127.0.0.1:8088`).

Gateway economics and runtime configuration live in the repository-root
`config/` files and are read by the Rust gateway, not the browser. The terminal
displays configuration and policy status read-only; it cannot edit them.

## Layout

- `app/page.tsx` — the operator console (client-rendered, fail-closed validation
  of every projection frame)
- `app/layout.tsx`, `app/globals.css` — shell and styling
- `scripts/run-terminal.sh` — `dev:full` launcher for gateway + dashboard
- `tests/rendered-html.test.mjs` — asserts the shell fails closed before any
  gateway data is available
