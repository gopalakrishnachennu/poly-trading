"use client";

import { useEffect, useMemo, useRef, useState } from "react";

type AssetName = "BTC" | "ETH";
type Mode = "discovering" | "ready" | "stale" | "halted";

type Level = { price_micros: string; quantity_micros: string };
type Book = {
  token_id: string;
  condition_id: string;
  source_timestamp_ms: number;
  received_at_ms: number;
  hash: string;
  tick_size_micros: string;
  bids: Level[];
  asks: Level[];
  best_bid_micros: string;
  best_ask_micros: string;
};
type AssetProjection = {
  asset: AssetName;
  symbol: string;
  title: string;
  event_slug: string;
  condition_id: string;
  market_id: string;
  rules_fingerprint: string;
  resolution_source: string;
  start_time_ms: number;
  end_time_ms: number;
  reference_price_micros: string;
  target_price_micros: string;
  reference_received_at_ms: number;
  up_book: Book;
  down_book: Book;
  pair: {
    buy_pair_cost_micros: string;
    raw_gap_micros: string;
    executable_quantity_micros: string;
    observation: string;
    decision: "no_trade";
    reason: string;
  };
  feed: {
    market_identity: string;
    up_book: string;
    down_book: string;
    reference: string;
    age_ms: number;
  };
};
type Snapshot = {
  schema_version: 1;
  sequence: number;
  generated_at_ms: number;
  last_success_at_ms: number | null;
  mode: Mode;
  no_trade: true;
  reason: string;
  assets: AssetProjection[];
  credentials_present: false;
  authenticated_transport_present: false;
  order_submission_present: false;
  financial_authority_present: false;
  snapshot_digest: string;
};
type PaperTrade = { trade_id: string; asset: AssetName; condition_id: string; state: string; quantity_micros: string; up_price_micros: string; down_price_micros: string; fee_micros: string; slippage_micros: string; cost_micros: string; locked_pnl_micros: string; decision_at_ms: number };
type PaperContract = { asset: string; active: boolean; observations: number; no_trade: number; fills: number; realized_pnl_micros: string; last_decision: string; last_reason: string; last_event_ms: number | null };
type PaperStatus = { paper_only: true; active: boolean; session_id: string | null; started_at_ms: number | null; stopped_at_ms: number | null; deadline_at_ms: number | null; principal_micros: string; backup_micros: string; available_cash_micros: string; reserved_micros: string; realized_pnl_micros: string; locked_pnl_micros: string; unrealized_pnl_micros: string; max_drawdown_micros: string; cvar_micros: string; hedge_failures: number; fill_rate_bps: number; data_coverage_bps: number; events_recorded: number; decisions_recorded: number; checkpoints: number; last_checkpoint_ms: number | null; replay_digest: string; journal_path: string | null; last_error: string | null; policy_status: string; policy_id: string | null; policy_digest: string | null; contracts: PaperContract[]; trades: PaperTrade[]; daily_rollups: { day_utc: string; events: number; decisions: number; fills: number; no_trade: number; realized_pnl_micros: string; fees_micros: string; drawdown_micros: string }[] };
type PaperReport = { paper_only: true; campaign_id: string | null; replay_verified: boolean; journal_records: number; final_digest: string; net_pnl_micros: string; data_coverage_bps: number; verified_at_ms: number; gates: Record<string, boolean>; reason: string };
type PaperPreflight = { eligible: boolean; checked_at_ms: number; gates: Record<string, boolean>; policy_id: string | null; policy_digest: string | null; journal_directory: string; reason: string };
type RuntimeConfiguration = { mode: string; config_id: string | null; digest: string | null; source: string; restart_required_for_change: true; permits_new_paper_campaign: boolean; reason: string; effective: { sources: { gamma_keyset_url: string; clob_book_url: string; reference_api_url: string }; polling: { http_timeout_ms: number; refresh_interval_ms: number; discovery_refresh_ms: number; discovery_lookback_ms: number; discovery_lookahead_ms: number; maximum_response_bytes: number }; projection: { maximum_book_age_ms: number; maximum_reference_age_ms: number; maximum_cross_book_skew_ms: number; maximum_projection_age_ms: number }; client_display: { poll_interval_ms: number; request_timeout_ms: number; maximum_client_age_ms: number; maximum_future_skew_ms: number } } };
type ResearchExportStatus = { available: boolean; root: string; last_exported_at_ms: number | null; campaign_id: string | null; source_records: number; partitions: number; last_error: string | null };
type ResearchExportReport = { status: ResearchExportStatus; observation_rows: number; decision_rows: number; trade_rows: number };

const API = (process.env.NEXT_PUBLIC_TERMINAL_API_URL ?? "http://127.0.0.1:8088").replace(/\/+$/, "");
// Bootstrap-only guards apply before the read-only configuration frame loads.
const BOOTSTRAP_POLL_MS = 1_000;
const BOOTSTRAP_REQUEST_TIMEOUT_MS = 3_500;
const BOOTSTRAP_MAX_CLIENT_AGE_MS = 7_000;
// A projection produced by a clock that is ahead of the browser must not be
// treated as fresh indefinitely. Keep this deliberately small; the gateway
// already performs its own clock-integrity checks.
const BOOTSTRAP_MAX_FUTURE_SKEW_MS = 1_500;
// A full replay audit reads the journal. Keep it materially slower than the
// public-feed status loop so an operator opening several tabs cannot degrade
// the recorder or make the market display flicker.
const PAPER_AUDIT_POLL_FACTOR = 15;

function isIntegerString(value: unknown): value is string {
  return typeof value === "string" && /^-?\d+$/.test(value);
}

function isBook(value: unknown): value is Book {
  if (!value || typeof value !== "object") return false;
  const book = value as Partial<Book>;
  const validInteger = (raw: unknown, minimum: bigint, maximum: bigint) => {
    if (!isIntegerString(raw)) return false;
    try {
      const amount = BigInt(raw);
      return amount >= minimum && amount <= maximum;
    } catch { return false; }
  };
  const validPrice = (raw: unknown) => validInteger(raw, 0n, 1_000_000n);
  // Quantities are token millionths backed by i64 in the Rust boundary and
  // are not capped at one token. Large visible book sizes are valid.
  const validQuantity = (raw: unknown, allowZero = true) =>
    validInteger(raw, allowZero ? 0n : 1n, 9_223_372_036_854_775_807n);
  const levelsOkay = (levels: unknown, descending: boolean) =>
    Array.isArray(levels) && levels.length > 0 && levels.length <= 10 &&
    levels.every((level) => level && typeof level === "object" &&
      validPrice((level as Partial<Level>).price_micros) &&
      validQuantity((level as Partial<Level>).quantity_micros, false)) &&
    levels.every((level, index) => index === 0 || (descending
      ? BigInt((level as Level).price_micros) <= BigInt((levels[index - 1] as Level).price_micros)
      : BigInt((level as Level).price_micros) >= BigInt((levels[index - 1] as Level).price_micros)));
  return typeof book.token_id === "string" && book.token_id.length > 0 &&
    typeof book.condition_id === "string" && book.condition_id.length > 0 &&
    typeof book.source_timestamp_ms === "number" && Number.isSafeInteger(book.source_timestamp_ms) &&
    book.source_timestamp_ms >= 0 &&
    typeof book.received_at_ms === "number" && Number.isSafeInteger(book.received_at_ms) &&
    book.received_at_ms >= book.source_timestamp_ms &&
    // The venue's book hash is opaque (fixtures and providers may use a
    // non-hex digest), so validate presence/size here and let the gateway
    // verify its semantics.
    typeof book.hash === "string" && book.hash.length > 0 && book.hash.length <= 256 &&
    validPrice(book.tick_size_micros) && BigInt(book.tick_size_micros ?? "0") > 0n &&
    validPrice(book.best_bid_micros) && validPrice(book.best_ask_micros) &&
    levelsOkay(book.bids, true) && levelsOkay(book.asks, false);
}

function decodeSnapshot(value: unknown): Snapshot {
  if (!value || typeof value !== "object") throw new Error("projection is not an object");
  const item = value as Partial<Snapshot>;
  if (item.schema_version !== 1 || !["discovering", "ready", "stale", "halted"].includes(item.mode ?? "")) {
    throw new Error("unsupported projection schema or mode");
  }
  if (item.no_trade !== true || item.credentials_present !== false ||
      item.authenticated_transport_present !== false || item.order_submission_present !== false ||
      item.financial_authority_present !== false) {
    throw new Error("projection authority contract violated");
  }
  if (!Array.isArray(item.assets) || !Number.isSafeInteger(item.generated_at_ms) ||
      !Number.isSafeInteger(item.sequence) || (item.sequence ?? -1) < 0 ||
      typeof item.reason !== "string" || !/^[0-9a-f]{64}$/.test(item.snapshot_digest ?? "")) {
    throw new Error("projection envelope invalid");
  }
  if (item.mode === "ready") {
    const names = item.assets.map((asset) => asset.asset).sort().join(",");
    if (names !== "BTC,ETH") throw new Error("atomic BTC/ETH asset set missing");
    for (const asset of item.assets) {
      if (!["BTC", "ETH"].includes(asset.asset) || !Number.isSafeInteger(asset.start_time_ms) ||
          !Number.isSafeInteger(asset.end_time_ms) || asset.start_time_ms >= asset.end_time_ms ||
          !isIntegerString(asset.reference_price_micros) ||
          !isIntegerString(asset.target_price_micros) || !isBook(asset.up_book) || !isBook(asset.down_book) ||
          asset.up_book.condition_id !== asset.condition_id || asset.down_book.condition_id !== asset.condition_id ||
          asset.pair?.decision !== "no_trade" || !isIntegerString(asset.pair.buy_pair_cost_micros) ||
          !isIntegerString(asset.pair.raw_gap_micros) || !isIntegerString(asset.pair.executable_quantity_micros)) {
        throw new Error("asset projection invalid");
      }
    }
  } else if (item.assets.length !== 0) {
    throw new Error("unavailable projection retained assets");
  }
  return item as Snapshot;
}

function decimal(micros?: string, digits = 3): string {
  if (!micros || !/^\d+$/.test(micros)) return "UNAVAILABLE";
  const value = BigInt(micros);
  const whole = value / 1_000_000n;
  const fraction = (value % 1_000_000n).toString().padStart(6, "0").slice(0, digits);
  return `${whole.toLocaleString()}${digits ? `.${fraction}` : ""}`;
}

function signedDecimal(micros?: string): string {
  if (!micros || !/^-?\d+$/.test(micros)) return "UNAVAILABLE";
  const value = BigInt(micros);
  const sign = value > 0n ? "+" : value < 0n ? "−" : "";
  return `${sign}${decimal((value < 0n ? -value : value).toString())}`;
}

function usd(micros?: string, digits = 2): string {
  const value = decimal(micros, digits);
  return value === "UNAVAILABLE" ? value : `$${value}`;
}

function signedUsd(micros?: string): string {
  const value = signedDecimal(micros);
  return value === "UNAVAILABLE" ? value : `${value.startsWith("−") ? "−" : value.startsWith("+") ? "+" : ""}$${value.replace(/^[+−]/, "")}`;
}

function validMicros(value?: string): bigint {
  return value && /^-?\d+$/.test(value) ? BigInt(value) : 0n;
}

function quantity(micros?: string): string {
  if (!micros || !/^\d+$/.test(micros)) return "—";
  return (Number(BigInt(micros) / 1_000n) / 1_000).toLocaleString(undefined, { maximumFractionDigits: 3 });
}

function short(value?: string | null): string {
  return value && value.length > 13 ? `${value.slice(0, 7)}…${value.slice(-5)}` : value || "UNAVAILABLE";
}

function utcTime(ms?: number): string {
  return typeof ms === "number" ? new Date(ms).toISOString().slice(11, 23) : "--:--:--.---";
}

function parseUsdMicros(value: string): string {
  const trimmed = value.trim().replace(/^\$/, "");
  if (!/^\d+(\.\d{0,6})?$/.test(trimmed)) throw new Error("amount must be a non-negative USD value");
  const [whole, fraction = ""] = trimmed.split(".");
  const micros = BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0") || "0");
  if (micros > 9_000_000_000_000_000_000n) throw new Error("amount exceeds paper limit");
  return micros.toString();
}

function PaperNeuralField({ status }: { status: PaperStatus | null }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const node = canvas.current;
    if (!node) return;
    const ratio = window.devicePixelRatio || 1;
    const width = node.clientWidth || 420;
    const height = node.clientHeight || 84;
    node.width = width * ratio; node.height = height * ratio;
    const ctx = node.getContext("2d"); if (!ctx) return;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const nodes = 16;
    const phase = (status?.events_recorded ?? 0) * 0.09;
    const points = Array.from({ length: nodes }, (_, index) => ({ x: 12 + index * (width - 24) / (nodes - 1), y: height / 2 + Math.sin(index * 1.7 + phase) * (height * 0.27) }));
    ctx.lineWidth = 1;
    points.forEach((point, index) => {
      if (index === 0) return;
      ctx.strokeStyle = index % 3 === 0 ? "rgba(255,173,24,.42)" : "rgba(57,199,255,.30)";
      ctx.beginPath(); ctx.moveTo(points[index - 1].x, points[index - 1].y); ctx.lineTo(point.x, point.y); ctx.stroke();
    });
    points.forEach((point, index) => { ctx.fillStyle = index % 3 === 0 ? "#ffad18" : "#39c7ff"; ctx.beginPath(); ctx.arc(point.x, point.y, index % 3 === 0 ? 3 : 2, 0, Math.PI * 2); ctx.fill(); });
  }, [status]);
  return <canvas className="neural-field" ref={canvas} role="img" aria-label="Paper strategy decision telemetry field" />;
}

function Panel({ code, title, action, children, className = "" }: {
  code: string; title: string; action?: React.ReactNode; children: React.ReactNode; className?: string;
}) {
  return <section className={`panel ${className}`}>
    <header className="panel-head"><div><span className="panel-code">{code}</span><h2>{title}</h2></div>{action && <div className="panel-action">{action}</div>}</header>
    <div className="panel-body">{children}</div>
  </section>;
}

function StatusDot({ tone = "green" }: { tone?: "green" | "amber" | "red" | "blue" }) {
  return <span className={`status-dot ${tone}`} aria-hidden="true" />;
}

function MiniBook({ side, book }: { side: "UP" | "DOWN"; book?: Book }) {
  const asks = book?.asks.slice(0, 2) ?? [];
  const bids = book?.bids.slice(0, 3) ?? [];
  const rows = [...asks, ...bids];
  const max = rows.reduce((current, row) => {
    const size = Number(BigInt(row.quantity_micros) / 1_000n);
    return Math.max(current, size);
  }, 1);
  return <div className="book-side">
    <div className="book-title"><span className={side === "UP" ? "positive" : "negative"}>{side}</span><span>PRICE</span><span>SIZE</span></div>
    {rows.length === 0 ? <div className="book-empty">AUTHORITATIVE BOOK UNAVAILABLE</div> : rows.map((row, index) => {
      const size = Number(BigInt(row.quantity_micros) / 1_000n);
      return <div className="book-row" key={`${side}-${row.price_micros}-${index}`}>
        <div className={`depth ${side.toLowerCase()}`} style={{ width: `${Math.max(1, size / max * 100)}%` }} />
        <span>{index < asks.length ? "ASK" : "BID"}</span><strong>{decimal(row.price_micros)}</strong><span>{quantity(row.quantity_micros)}</span>
      </div>;
    })}
  </div>;
}

function LiveChart({ points, target }: { points: string[]; target?: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const node = canvas.current;
    if (!node) return;
    const ratio = window.devicePixelRatio || 1;
    const width = node.clientWidth;
    const height = node.clientHeight;
    node.width = Math.max(1, Math.floor(width * ratio));
    node.height = Math.max(1, Math.floor(height * ratio));
    const context = node.getContext("2d");
    if (!context) return;
    context.scale(ratio, ratio);
    context.clearRect(0, 0, width, height);
    context.strokeStyle = "#1a2429";
    context.lineWidth = 1;
    for (let index = 1; index < 5; index += 1) {
      context.beginPath(); context.moveTo(0, height * index / 5); context.lineTo(width, height * index / 5); context.stroke();
    }
    const values = points.map(BigInt);
    if (target && /^\d+$/.test(target)) values.push(BigInt(target));
    if (values.length < 2) return;
    const minimum = values.reduce((a, b) => a < b ? a : b);
    const maximum = values.reduce((a, b) => a > b ? a : b);
    const range = maximum === minimum ? 1n : maximum - minimum;
    const y = (value: bigint) => height - 18 - Number((value - minimum) * BigInt(Math.max(1, Math.floor(height - 36))) / range);
    if (target && /^\d+$/.test(target)) {
      context.setLineDash([5, 4]); context.strokeStyle = "#ffad18"; context.beginPath(); context.moveTo(0, y(BigInt(target))); context.lineTo(width, y(BigInt(target))); context.stroke(); context.setLineDash([]);
    }
    if (points.length > 1) {
      context.strokeStyle = "#39c7ff"; context.lineWidth = 2; context.beginPath();
      points.forEach((point, index) => {
        const x = index * width / (points.length - 1);
        const py = y(BigInt(point));
        if (index === 0) context.moveTo(x, py);
        else context.lineTo(x, py);
      });
      context.stroke();
    }
  }, [points, target]);
  return <canvas className="live-canvas" ref={canvas} role="img" aria-label="Received reference-price history" />;
}

export default function Terminal() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState("awaiting read-only projection gateway");
  const [selected, setSelected] = useState<AssetName>("BTC");
  const [paused, setPaused] = useState(false);
  const [localKill, setLocalKill] = useState(false);
  const [workspace, setWorkspace] = useState("MARKETS");
  const [query, setQuery] = useState("");
  // Keep the server and first client render deterministic; the clock starts
  // after hydration so the terminal does not throw away its SSR tree.
  const [now, setNow] = useState(0);
  const [history, setHistory] = useState<Record<AssetName, string[]>>({ BTC: [], ETH: [] });
  const [paper, setPaper] = useState<PaperStatus | null>(null);
  const [paperReport, setPaperReport] = useState<PaperReport | null>(null);
  const [paperPreflight, setPaperPreflight] = useState<PaperPreflight | null>(null);
  const [runtimeConfiguration, setRuntimeConfiguration] = useState<RuntimeConfiguration | null>(null);
  const [researchExport, setResearchExport] = useState<ResearchExportStatus | null>(null);
  const [paperStatusError, setPaperStatusError] = useState("");
  const [paperAuditError, setPaperAuditError] = useState("");
  const [principal, setPrincipal] = useState("1000");
  const [backup, setBackup] = useState("0");
  const [paperMessage, setPaperMessage] = useState("");
  const lastSequence = useRef<number | null>(null);
  const displayPolicy = runtimeConfiguration?.effective.client_display;
  const pollMs = displayPolicy?.poll_interval_ms ?? BOOTSTRAP_POLL_MS;
  const requestTimeoutMs = displayPolicy?.request_timeout_ms ?? BOOTSTRAP_REQUEST_TIMEOUT_MS;
  const maximumClientAgeMs = displayPolicy?.maximum_client_age_ms ?? BOOTSTRAP_MAX_CLIENT_AGE_MS;
  const maximumFutureSkewMs = displayPolicy?.maximum_future_skew_ms ?? BOOTSTRAP_MAX_FUTURE_SKEW_MS;
  const paperAuditPollMs = pollMs * PAPER_AUDIT_POLL_FACTOR;

  useEffect(() => {
    const initial = window.setTimeout(() => setNow(Date.now()), 0);
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => { window.clearTimeout(initial); window.clearInterval(timer); };
  }, []);

  useEffect(() => {
    if (paused) return;
    let disposed = false;
    let timer: number | undefined;
    let activeController: AbortController | undefined;
    const poll = async () => {
      const controller = new AbortController();
      activeController = controller;
      const timeout = window.setTimeout(() => controller.abort(), requestTimeoutMs);
      try {
        const response = await fetch(`${API}/api/v1/terminal/snapshot`, { cache: "no-store", signal: controller.signal });
        if (!response.ok) throw new Error(`gateway HTTP ${response.status}`);
        const next = decodeSnapshot(await response.json());
        const age = Date.now() - next.generated_at_ms;
        if (age > maximumClientAgeMs) throw new Error("projection exceeded client freshness budget");
        if (age < -maximumFutureSkewMs) throw new Error("projection clock is ahead of client");
        // A lower sequence is only acceptable while the gateway is rebuilding
        // after a restart (unavailable modes carry no market assets). Never
        // allow a regressed ready projection to replace a live one.
        if (lastSequence.current !== null && next.sequence < lastSequence.current && next.mode === "ready") {
          throw new Error("projection sequence regressed");
        }
        if (!disposed) {
          lastSequence.current = next.sequence;
          // An unavailable/stale envelope is valid protocol state, but it is
          // not a replacement for the last verified market frame. Retain that
          // frame as context while making the terminal ineligible immediately.
          setSnapshot((current) => next.mode === "ready" || current === null ? next : current);
          setError(next.mode === "ready" ? "" : next.reason);
          if (next.mode === "ready") setHistory((prior) => {
            const copy = { ...prior };
            for (const asset of next.assets) copy[asset.asset] = [...copy[asset.asset], asset.reference_price_micros].slice(-120);
            return copy;
          });
        }
      } catch (reason) {
        // Keep the last verified frame visible while explicitly marking it stale.
        // Clearing it here causes a transient network blip to erase the operator's
        // context and can hide the last known sequence needed for reconciliation.
        if (!disposed) setError(reason instanceof Error ? reason.message : "gateway unavailable");
      } finally {
        window.clearTimeout(timeout);
        if (activeController === controller) activeController = undefined;
        if (!disposed) timer = window.setTimeout(poll, pollMs);
      }
    };
    void poll();
    return () => { disposed = true; if (timer) window.clearTimeout(timer); activeController?.abort(); };
  }, [maximumClientAgeMs, maximumFutureSkewMs, paused, pollMs, requestTimeoutMs]);

  useEffect(() => {
    let disposed = false;
    const readConfiguration = async () => {
      try {
        const response = await fetch(`${API}/api/v1/configuration`, { cache: "no-store" });
        if (!response.ok) throw new Error(`configuration gateway HTTP ${response.status}`);
        const next = await response.json() as RuntimeConfiguration;
        if (typeof next.mode !== "string" || !next.effective || next.restart_required_for_change !== true) throw new Error("configuration contract invalid");
        if (!disposed) setRuntimeConfiguration(next);
      } catch (reason) { if (!disposed) setPaperMessage(reason instanceof Error ? reason.message : "configuration unavailable"); }
    };
    void readConfiguration();
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let activeController: AbortController | undefined;
    const pollPaper = async () => {
      const controller = new AbortController();
      activeController = controller;
      const timeout = window.setTimeout(() => controller.abort(), requestTimeoutMs);
      try {
        const response = await fetch(`${API}/api/v1/paper/status`, { cache: "no-store", signal: controller.signal });
        if (!response.ok) throw new Error(`paper gateway HTTP ${response.status}`);
        const next = await response.json() as PaperStatus;
        if (next.paper_only !== true) throw new Error("paper authority contract violated");
        if (!disposed) { setPaper(next); setPaperStatusError(""); }
      } catch (reason) { if (!disposed) setPaperStatusError(reason instanceof Error ? reason.message : "paper status unavailable"); }
      finally {
        window.clearTimeout(timeout);
        if (activeController === controller) activeController = undefined;
        if (!disposed) timer = window.setTimeout(pollPaper, pollMs);
      }
    };
    void pollPaper();
    return () => { disposed = true; if (timer) window.clearTimeout(timer); activeController?.abort(); };
  }, [pollMs, requestTimeoutMs]);

  useEffect(() => {
    let disposed = false;
    const readPreflight = async () => {
      try {
        const response = await fetch(`${API}/api/v1/paper/preflight`, { cache: "no-store" });
        if (!response.ok) throw new Error(`paper preflight HTTP ${response.status}`);
        const next = await response.json() as PaperPreflight;
        if (typeof next.eligible !== "boolean" || !Number.isSafeInteger(next.checked_at_ms) || !next.gates) throw new Error("paper preflight contract invalid");
        if (!disposed) setPaperPreflight(next);
      } catch (reason) { if (!disposed) setPaperMessage(reason instanceof Error ? reason.message : "paper preflight unavailable"); }
    };
    void readPreflight();
    return () => { disposed = true; };
  }, [runtimeConfiguration?.digest, paper?.active]);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let activeController: AbortController | undefined;
    const pollAudit = async () => {
      const controller = new AbortController();
      activeController = controller;
      const timeout = window.setTimeout(() => controller.abort(), requestTimeoutMs);
      try {
        const [reportResponse, exportResponse] = await Promise.all([
          fetch(`${API}/api/v1/paper/report`, { cache: "no-store", signal: controller.signal }),
          fetch(`${API}/api/v1/research-export/status`, { cache: "no-store", signal: controller.signal }),
        ]);
        if (!reportResponse.ok) throw new Error(`paper report HTTP ${reportResponse.status}`);
        if (!exportResponse.ok) throw new Error(`research export HTTP ${exportResponse.status}`);
        const [report, exported] = await Promise.all([reportResponse.json() as Promise<PaperReport>, exportResponse.json() as Promise<ResearchExportStatus>]);
        if (report.paper_only !== true || !Number.isSafeInteger(report.verified_at_ms)) throw new Error("paper report authority contract violated");
        if (typeof exported.available !== "boolean" || typeof exported.root !== "string" || !Number.isSafeInteger(exported.source_records) || !Number.isSafeInteger(exported.partitions)) throw new Error("research export contract invalid");
        if (!disposed) { setPaperReport(report); setResearchExport(exported); setPaperAuditError(""); }
      } catch (reason) { if (!disposed) setPaperAuditError(reason instanceof Error ? reason.message : "paper audit unavailable"); }
      finally {
        window.clearTimeout(timeout);
        if (activeController === controller) activeController = undefined;
        if (!disposed) timer = window.setTimeout(pollAudit, paperAuditPollMs);
      }
    };
    void pollAudit();
    return () => { disposed = true; if (timer) window.clearTimeout(timer); activeController?.abort(); };
  }, [paperAuditPollMs, requestTimeoutMs]);

  const startPaper = async () => {
    try {
      const response = await fetch(`${API}/api/v1/paper/session`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ principal_micros: parseUsdMicros(principal), backup_micros: parseUsdMicros(backup), contracts: ["BTC", "ETH"] }) });
      const payload = await response.json() as PaperStatus & { error?: string };
      if (!response.ok) throw new Error(payload.error ?? `paper start HTTP ${response.status}`);
      setPaper(payload); setPaperStatusError(""); setPaperMessage("paper session started; no external orders");
    } catch (reason) { setPaperMessage(reason instanceof Error ? reason.message : "paper start failed"); }
  };
  const stopPaper = async () => {
    try {
      const response = await fetch(`${API}/api/v1/paper/session/stop`, { method: "POST" });
      if (!response.ok) throw new Error(`paper stop HTTP ${response.status}`);
      setPaper(await response.json() as PaperStatus); setPaperStatusError(""); setPaperMessage("paper session stopped; journal retained");
    } catch (reason) { setPaperMessage(reason instanceof Error ? reason.message : "paper stop failed"); }
  };
  const refreshResearchExport = async () => {
    try {
      const response = await fetch(`${API}/api/v1/research-export/refresh`, { method: "POST" });
      const payload = await response.json() as ResearchExportReport & { error?: string };
      if (!response.ok) throw new Error(payload.error ?? `research export HTTP ${response.status}`);
      if (!payload.status || typeof payload.status.available !== "boolean") throw new Error("research export response invalid");
      setResearchExport(payload.status);
      setPaperMessage(`research export refreshed: ${payload.observation_rows} observations / ${payload.decision_rows} decisions / ${payload.trade_rows} trades`);
    } catch (reason) { setPaperMessage(reason instanceof Error ? reason.message : "research export failed"); }
  };

  const clientFresh = snapshot ? now - snapshot.generated_at_ms <= maximumClientAgeMs && snapshot.generated_at_ms - now <= maximumFutureSkewMs : false;
  // A previously verified frame is useful context during a short outage, but
  // it must never remain eligible as a current frame after any failed poll.
  const ready = !paused && !error && clientFresh && snapshot?.mode === "ready" && snapshot.assets.length === 2;
  const asset = ready ? snapshot.assets.find((item) => item.asset === selected) : undefined;
  const mode: Mode | "offline" = paused
    ? "stale"
    : !snapshot
      ? "offline"
      : error || !clientFresh
        ? "stale"
        : snapshot.mode;
  const noTrade = true;
  const remaining = asset ? Math.max(0, asset.end_time_ms - now) : 0;
  const remainingText = asset ? `${String(Math.floor(remaining / 60_000)).padStart(2, "0")}:${String(Math.floor(remaining % 60_000 / 1_000)).padStart(2, "0")}` : "--:--";
  const statusTone: "green" | "amber" | "red" = ready && !paused ? "green" : mode === "halted" || mode === "offline" ? "red" : "amber";
  const reason = localKill ? "local operator display latch engaged" : error || snapshot?.reason || "projection unavailable";
  const visibleAssets = ready ? snapshot.assets : [];
  const audit = useMemo(() => [
    [utcTime(snapshot?.generated_at_ms), "PROJECTION", `Sequence ${snapshot?.sequence ?? "—"} · ${mode.toUpperCase()}`],
    [utcTime(asset?.up_book.received_at_ms), "UP BOOK", asset ? `${asset.asset} best ask ${decimal(asset.up_book.best_ask_micros)}` : "unavailable"],
    [utcTime(asset?.down_book.received_at_ms), "DOWN BOOK", asset ? `${asset.asset} best ask ${decimal(asset.down_book.best_ask_micros)}` : "unavailable"],
    [utcTime(asset?.reference_received_at_ms), "REFERENCE", asset ? `${asset.symbol} ${decimal(asset.reference_price_micros, 2)}` : "unavailable"],
  ], [snapshot, asset, mode]);
  const paperPrincipal = validMicros(paper?.principal_micros);
  const paperBackup = validMicros(paper?.backup_micros);
  const pendingPayout = paper?.trades.reduce((total, trade) => total + (trade.state === "FILLED_PAIR_LOCKED" ? validMicros(trade.quantity_micros) : 0n), 0n) ?? 0n;
  const pendingCost = paper?.trades.reduce((total, trade) => total + (trade.state === "FILLED_PAIR_LOCKED" ? validMicros(trade.cost_micros) : 0n), 0n) ?? 0n;
  const capitalAssigned = paperPrincipal + paperBackup;
  const replayHealthy = paperReport?.replay_verified === true && Object.values(paperReport.gates).every(Boolean);
  const paperState = paper === null ? "RECONCILING" : paper.active ? "RUNNING" : "IDLE";
  const paperStateDetail = paperStatusError || paperAuditError || paperMessage || (paper === null ? "awaiting paper-status authority" : "one-week evidence mode");

  return <main className={localKill ? "terminal kill-active" : "terminal"}>
    <header className="topbar">
      <div className="brand-block"><div className="brand-mark">PT</div><div><strong>POLY//TERMINAL</strong><span>READ-ONLY MARKET CONTROL</span></div></div>
      <div className="command-wrap"><span className="command-key">CMD</span><input aria-label="Terminal command" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search display…"/><kbd>⌘ K</kbd></div>
      <div className="top-status"><div><span>UTC</span><strong>{utcTime(now)}</strong></div><div><span>MODE</span><strong className="amber-text">PUBLIC / READ ONLY</strong></div><button className={localKill ? "kill-button active" : "kill-button"} onClick={() => setLocalKill((value) => !value)}>{localKill ? "LOCAL LATCHED" : "LOCAL NO_TRADE"}</button></div>
    </header>
    <nav className="function-bar" aria-label="Terminal workspaces">
      {["MARKETS", "RISK", "POSITIONS", "SETTLEMENT", "OPERATIONS", "AUDIT", "SETTINGS"].map((item, index) => <button key={item} className={workspace === item ? "active" : ""} onClick={() => setWorkspace(item)}><span>F{index + 1}</span>{item}</button>)}
      <div className="function-spacer"/><span className="connection"><StatusDot tone={statusTone}/>{paused ? "CLIENT PAUSED" : `${mode.toUpperCase()} · PUBLIC FEED`}</span>
    </nav>
    {(!ready || localKill || noTrade) && <div className={`kill-banner ${mode === "halted" ? "halted" : ""}`}><strong>GLOBAL NO_TRADE</strong><span>{reason}</span></div>}
    <div className="workspace-label"><span>W1</span>{workspace} / CURRENT BTC + ETH HOURLY MARKETS <em>LIVE PUBLIC PROJECTION</em></div>
    <section className="paper-control-strip" aria-label="Paper week controls">
      <div className="paper-control-title"><span className="panel-code">P0</span><div><strong>PAPER WEEK RUNNER</strong><small>LOCAL SIMULATION · PUBLIC FEEDS · NO EXTERNAL ORDERS</small></div></div>
      <label>PRINCIPAL USD<input value={principal} onChange={(event) => setPrincipal(event.target.value)} inputMode="decimal" disabled={paper?.active === true} /></label>
      <label>BACKUP USD<input value={backup} onChange={(event) => setBackup(event.target.value)} inputMode="decimal" disabled={paper?.active === true} /></label>
      <button className="paper-start" onClick={() => void startPaper()} disabled={paper === null || paper.active === true || paused || localKill || paperPreflight?.eligible !== true}>START PAPER</button>
      <button className="paper-stop" onClick={() => void stopPaper()} disabled={paper?.active !== true}>STOP PAPER</button>
      <div className="paper-state"><StatusDot tone={paper?.active ? "green" : "amber"}/>{paperState}<small>{paperStateDetail}</small></div>
    </section>

    {workspace === "SETTINGS" && <section className="configuration-section" aria-label="Effective configuration">
      <div className="configuration-head"><span className="panel-code">C1</span><div><strong>CONFIGURATION &amp; POLICY REGISTRY</strong><small>READ ONLY · CHANGES REQUIRE VALIDATION AND GATEWAY RESTART</small></div><b className={runtimeConfiguration?.mode === "BOUND" ? "positive" : "amber-text"}>{runtimeConfiguration?.mode ?? "UNAVAILABLE"}</b></div>
      {runtimeConfiguration ? <div className="configuration-grid">
        <div><span>CONFIG ID</span><b>{runtimeConfiguration.config_id ?? "UNBOUND"}</b><span>DIGEST</span><b>{short(runtimeConfiguration.digest)}</b><span>SOURCE</span><b>{runtimeConfiguration.source}</b></div>
        <div><span>GAMMA</span><b>{runtimeConfiguration.effective.sources.gamma_keyset_url}</b><span>CLOB</span><b>{runtimeConfiguration.effective.sources.clob_book_url}</b><span>REFERENCE</span><b>{runtimeConfiguration.effective.sources.reference_api_url}</b></div>
        <div><span>POLL / HTTP</span><b>{runtimeConfiguration.effective.polling.refresh_interval_ms}ms / {runtimeConfiguration.effective.polling.http_timeout_ms}ms</b><span>DISCOVERY</span><b>{runtimeConfiguration.effective.polling.discovery_refresh_ms}ms · ±{runtimeConfiguration.effective.polling.discovery_lookback_ms}ms</b><span>FRESHNESS</span><b>BOOK {runtimeConfiguration.effective.projection.maximum_book_age_ms}ms · REF {runtimeConfiguration.effective.projection.maximum_reference_age_ms}ms</b></div>
        <div className="configuration-no-trade"><strong>NO SILENT LIVE EDITS</strong><span>{runtimeConfiguration.reason}</span><span>NEW PAPER: {runtimeConfiguration.permits_new_paper_campaign ? "CONFIGURATION ELIGIBLE" : "BLOCKED"}</span></div>
      </div> : <div className="configuration-empty">Configuration gateway unavailable. Browser remains fail-closed.</div>}
      <div className="research-export-panel">
        <div><span>PAPER CAMPAIGN PREFLIGHT</span><b>{paperPreflight?.eligible ? "READY FOR EXPLICIT START" : "START BLOCKED"}</b><small>BOOT NEVER STARTS OR RESUMES A RECORDER; CONFIGURATION, POLICY, CLOCK, JOURNAL AND OPERATOR ACTION ARE SEPARATE GATES.</small></div>
        <div><span>POLICY</span><b>{paperPreflight?.policy_id ?? "UNAVAILABLE"}</b><span>DIGEST</span><b>{short(paperPreflight?.policy_digest)}</b></div>
        <div><span>JOURNAL DIRECTORY</span><b>{paperPreflight?.journal_directory ?? "UNAVAILABLE"}</b><span>CHECKED UTC</span><b>{paperPreflight ? utcTime(paperPreflight.checked_at_ms) : "—"}</b></div>
        <div className="configuration-no-trade"><strong>{paperPreflight?.eligible ? "EXPLICIT START REQUIRED" : "NO_TRADE"}</strong><span>{paperPreflight?.reason ?? "Awaiting preflight authority."}</span><span>{paperPreflight ? Object.entries(paperPreflight.gates).filter(([, passed]) => !passed).map(([gate]) => gate).join(", ") || "all gates passed" : "unavailable"}</span></div>
      </div>
      <div className="research-export-panel">
        <div><span>RESEARCH EXPORT</span><b>{researchExport?.available ? "VERIFIED DERIVED VIEWS" : "NOT EXPORTED"}</b><small>CSV + PARQUET · ASSET / UTC DATE / UTC HOUR · SOURCE JSONL REMAINS AUTHORITATIVE</small></div>
        <div><span>ROOT</span><b>{researchExport?.root ?? "UNAVAILABLE"}</b><span>CAMPAIGN / RECORDS</span><b>{researchExport?.campaign_id ?? "—"} / {researchExport?.source_records ?? 0}</b></div>
        <div><span>PARTITIONS</span><b>{researchExport?.partitions ?? 0}</b><span>LAST EXPORT UTC</span><b>{researchExport?.last_exported_at_ms ? utcTime(researchExport.last_exported_at_ms) : "—"}</b></div>
        <div className="research-export-action"><button onClick={() => void refreshResearchExport()} disabled={!paper?.journal_path}>REFRESH LOCAL EXPORT</button><small>{researchExport?.last_error ?? "Manual export only; no upload, credentials, or trading authority."}</small></div>
      </div>
    </section>}

    <div className="terminal-grid">
      <Panel code="01" title="MARKET IDENTITY" className="watch-panel" action={<span>{ready ? "2 VALIDATED" : "0 AUTHORIZED"}</span>}>
        <div className="watch-head"><span>CONTRACT</span><span>REFERENCE</span><span>UP ASK</span></div>
        {visibleAssets.map((item) => <button className={`watch-row ${selected === item.asset ? "selected" : ""}`} key={item.asset} onClick={() => setSelected(item.asset)}><span><b>{item.asset}</b><small>{new Date(item.start_time_ms).toISOString().slice(11,16)}–{new Date(item.end_time_ms).toISOString().slice(11,16)} UTC</small></span><span><strong>{decimal(item.reference_price_micros, 2)}</strong><small>PUBLIC REF</small></span><span><strong>{decimal(item.up_book.best_ask_micros)}</strong><small>CLOB</small></span></button>)}
        {!ready && <div className="book-empty">NO COMPLETE CURRENT BTC/ETH PROJECTION<br/>{reason}</div>}
        <div className="session-block"><div className="section-label">SESSION CLOCK</div><div className="countdown">{remainingText}<span>REMAINING</span></div><div className="session-line"><span>Open</span><b>{utcTime(asset?.start_time_ms).slice(0,8)}</b></div><div className="session-line"><span>Close</span><b>{utcTime(asset?.end_time_ms).slice(0,8)}</b></div><div className="session-line"><span>Condition</span><b>{short(asset?.condition_id)}</b></div><div className="session-line"><span>Rules hash</span><b>{short(asset?.rules_fingerprint)}</b></div></div>
        <div className="compact-health"><div><StatusDot tone={ready ? "green" : "red"}/>MARKET IDENTITY</div><div><StatusDot tone={ready ? "green" : "red"}/>UP BOOK</div><div><StatusDot tone={ready ? "green" : "red"}/>DOWN BOOK</div><div><StatusDot tone={ready ? "green" : "red"}/>REFERENCE</div></div>
      </Panel>

      <Panel code="02" title={`${asset?.symbol ?? selected} / REFERENCE`} className="chart-panel" action={asset && <a className="market-link" href={`https://polymarket.com/event/${asset.event_slug}`} target="_blank" rel="noreferrer">OPEN MARKET ↗</a>}>
        <div className="quote-strip"><div><span>REFERENCE</span><strong>{decimal(asset?.reference_price_micros, 2)}</strong></div><div><span>HOUR TARGET</span><strong>{decimal(asset?.target_price_micros, 2)}</strong></div><div><span>UP BEST BID</span><strong>{decimal(asset?.up_book.best_bid_micros)}</strong></div><div><span>UP BEST ASK</span><strong>{decimal(asset?.up_book.best_ask_micros)}</strong></div><div><span>FEED AGE</span><strong>{asset ? `${asset.feed.age_ms}ms` : "UNAVAILABLE"}</strong></div></div>
        <div className="chart"><LiveChart points={history[selected]} target={asset?.target_price_micros}/><div className="target-line"><span>VALIDATED HOURLY OPEN</span></div><div className="chart-x"><span>CLIENT HISTORY</span><span>RECEIVED VALUES ONLY</span><span>MAX 120 SAMPLES</span></div></div>
        <div className="probability-band"><div className="prob-up" style={{ width: asset ? `${Number(BigInt(asset.up_book.best_ask_micros)) / 10_000}%` : "50%" }}><span>UP ASK</span><strong>{decimal(asset?.up_book.best_ask_micros)}</strong></div><div className="prob-down"><strong>{decimal(asset?.down_book.best_ask_micros)}</strong><span>DOWN ASK</span></div></div>
      </Panel>

      <Panel code="03" title="COMPLEMENTARY BOOK" className="book-panel" action={<span>{asset ? `TICK ${decimal(asset.up_book.tick_size_micros)}` : "UNAVAILABLE"}</span>}>
        <MiniBook side="UP" book={asset?.up_book}/><MiniBook side="DOWN" book={asset?.down_book}/>
        <div className="pair-economics"><div><span>RAW BUY PAIR</span><b>{decimal(asset?.pair.buy_pair_cost_micros)}</b></div><div><span>RAW GAP TO 1</span><b>{signedDecimal(asset?.pair.raw_gap_micros)}</b></div><div><span>EXECUTABLE QTY</span><b>{quantity(asset?.pair.executable_quantity_micros)}</b></div><div className="decision"><span>OBSERVATION ONLY</span><b>NO_TRADE</b></div></div>
      </Panel>

      <Panel code="04" title="PAPER CAPITAL & RISK" className="risk-panel" action={<span className="badge-safe unavailable">SIMULATED ONLY</span>}>
        <div className="risk-grid"><div className="capital-gauge"><div className="gauge-ring unavailable"><div><strong>{paper ? usd(capitalAssigned.toString(), 0) : "—"}</strong><span>PAPER ALLOCATION</span></div></div><div className="floor-label"><span>LIVE CAPITAL FLOOR</span><b>NOT CONNECTED</b></div></div><div className="risk-metrics"><div><span>Principal allocation</span><b>{usd(paper?.principal_micros)}</b></div><div><span>Backup reserve</span><b>{usd(paper?.backup_micros)}</b></div><div><span>Available paper cash</span><b>{usd(paper?.available_cash_micros)}</b></div><div><span>Open reservation</span><b>{usd(paper?.reserved_micros)}</b></div><div><span>Locked / realized P&amp;L</span><b>{signedUsd(paper?.locked_pnl_micros)} / {signedUsd(paper?.realized_pnl_micros)}</b></div></div><div className="gate-stack"><div><StatusDot tone={paper?.active ? "green" : "amber"}/>PAPER SESSION {paper?.active ? "RUNNING" : "IDLE"}</div><div><StatusDot tone={paper?.policy_status === "BOUND" ? "green" : "amber"}/>PAPER POLICY {paper?.policy_status ?? "UNCONFIGURED"}</div><div><StatusDot tone={replayHealthy ? "green" : "amber"}/>REPLAY INTEGRITY {replayHealthy ? "VERIFIED" : "PENDING"}</div><div><StatusDot tone={paper?.data_coverage_bps === 10_000 ? "green" : "amber"}/>DATA COVERAGE {paper ? `${(paper.data_coverage_bps / 100).toFixed(2)}%` : "UNAVAILABLE"}</div><div><StatusDot tone="blue"/>NO WALLET, SIGNER OR ORDER TRANSPORT</div><div className="no-trade">NO_TRADE<span>{paper?.policy_status === "BOUND" ? "SIMULATED PAIRS REQUIRE A FRESH EXECUTABLE COMPLETE-SET EDGE; THIS NEVER AUTHORIZES LIVE EXPOSURE." : "POLICY IS NOT BOUND; OBSERVATION ONLY."}</span></div></div></div>
      </Panel>

      <Panel code="05" title="SIMULATED PAIRS & SETTLEMENT" className="positions-panel" action={<span>{paper?.trades.length ?? 0} PAPER PAIRS</span>}>
        <div className="table-head positions"><span>MARKET</span><span>QTY</span><span>COST</span><span>STATE</span><span>P&amp;L</span></div>{paper?.trades.length ? <div className="paper-position-table">{paper.trades.slice().reverse().slice(0, 4).map((trade) => <div className="paper-position-row" key={trade.trade_id}><strong>{trade.asset}</strong><span>{quantity(trade.quantity_micros)}</span><span>{usd(trade.cost_micros)}</span><span>{trade.state}</span><b>{signedUsd(trade.locked_pnl_micros)}</b></div>)}</div> : <div className="empty-position"><span>—</span><strong>NO SIMULATED PAIRS</strong><small>Conservative NO_TRADE decisions are recorded as evidence; no payout is assumed.</small></div>}<div className="settlement-row"><span><StatusDot tone="amber"/>PENDING PAYOUT {usd(pendingPayout.toString())}</span><span>COMMITTED {usd(pendingCost.toString())}</span><strong className="amber-text">NOT SPENDABLE</strong></div>
      </Panel>

      <Panel code="06" title="SYSTEM HEALTH" className="health-panel" action={<button className="text-button" onClick={() => setPaused((value) => !value)}>{paused ? "RESUME" : "PAUSE CLIENT"}</button>}>
        <div className="health-list"><div><span><StatusDot tone={statusTone}/>Projection gateway</span><b>{mode.toUpperCase()}</b></div><div><span><StatusDot tone={ready ? "green" : "red"}/>Atomic assets</span><b>{ready ? "BTC + ETH" : "NONE"}</b></div><div><span><StatusDot tone={paper?.active ? "green" : "amber"}/>Paper recorder</span><b>{paper?.active ? `${paper.events_recorded} EVENTS` : "IDLE"}</b></div><div><span><StatusDot tone={replayHealthy ? "green" : "amber"}/>Journal replay</span><b>{replayHealthy ? "VERIFIED" : "PENDING"}</b></div><div><span><StatusDot tone="blue"/>Credentials / transport</span><b>ABSENT</b></div><div><span><StatusDot tone={snapshot?.no_trade ? "green" : "red"}/>NO_TRADE contract</span><b>{snapshot?.no_trade ? "ENFORCED" : "CLIENT FAIL-CLOSED"}</b></div><div><span><StatusDot tone={ready ? "green" : "amber"}/>Projection age</span><b>{snapshot ? `${Math.max(0, now - snapshot.generated_at_ms)}ms` : "UNAVAILABLE"}</b></div></div>
      </Panel>

      <Panel code="07" title="PROJECTION AUDIT TAPE" className="audit-panel" action={<span>SEQ {snapshot?.sequence ?? "—"}</span>}>
        <div className="audit-tape">{audit.map(([time, kind, message]) => <div key={kind}><time>{time}</time><b className={kind === "PROJECTION" ? "amber-text" : "positive"}>{kind}</b><span>{message}</span><em>public</em></div>)}<div><time>{utcTime(now)}</time><b className="negative">AUTHORITY</b><span>Credentials, signing, accounting and orders absent</span><em>boundary</em></div></div>
      </Panel>
    </div>
    <section className="paper-section" aria-label="Paper campaign telemetry">
      <div className="paper-section-head"><div><span className="panel-code">P1</span><h2>CAMPAIGN TELEMETRY / TRADES</h2></div><span>{paper?.session_id ?? "NO SESSION"} · {paper?.events_recorded ?? 0} EVENTS</span></div>
      <div className="paper-section-grid">
        <div className="paper-neural"><div className="section-label">DECISION / EVIDENCE FIELD <span>TELEMETRY ONLY</span></div><PaperNeuralField status={paper}/><div className="paper-metrics"><span>AVAILABLE CASH <b>{usd(paper?.available_cash_micros)}</b></span><span>UNREALIZED <b>{signedUsd(paper?.unrealized_pnl_micros)}</b></span><span>MAX DRAWDOWN <b>{usd(paper?.max_drawdown_micros)}</b></span><span>CVaR <b>{usd(paper?.cvar_micros)}</b></span><span>HEDGE FAILURES <b>{paper?.hedge_failures ?? "—"}</b></span><span>FILL RATE <b>{paper ? `${(paper.fill_rate_bps / 100).toFixed(2)}%` : "—"}</b></span><span>DATA COVERAGE <b>{paper ? `${(paper.data_coverage_bps / 100).toFixed(2)}%` : "—"}</b></span><span>CHECKPOINTS <b>{paper?.checkpoints ?? "—"}</b></span></div></div>
        <div className="paper-contracts"><div className="section-label">CONTRACT ACTIVITY <span>SEPARATE STREAMS</span></div>{paper?.contracts.length ? paper.contracts.map((contract) => <div className="contract-row" key={contract.asset}><strong>{contract.asset}</strong><span>{contract.last_decision}</span><span>{contract.observations} OBS</span><b>{signedDecimal(contract.realized_pnl_micros)}</b></div>) : <div className="paper-empty">START A PAPER SESSION TO RECORD BTC + ETH</div>}</div>
        <div className="paper-trades"><div className="section-label">SIMULATED TRADES <span>{paper?.trades.length ?? 0} RECENT</span></div>{paper?.trades.length ? <div className="trade-table">{paper.trades.slice().reverse().slice(0, 12).map((trade) => <div className="trade-row" key={trade.trade_id}><b>{trade.asset}</b><span>{trade.state}</span><span>{quantity(trade.quantity_micros)} pair</span><span>{decimal(trade.cost_micros)}</span><strong>{signedDecimal(trade.locked_pnl_micros)}</strong></div>)}</div> : <div className="paper-empty">NO FILLS — CONSERVATIVE NO_TRADE IS VALID</div>}</div>
      </div>
    </section>
    <footer className="statusbar"><span><kbd>F1</kbd> Markets</span><span><kbd>F2</kbd> Risk</span><span><kbd>⌘K</kbd> Display search</span><div/><strong>NO EXTERNAL ORDERS</strong><span>Digest {short(snapshot?.snapshot_digest)}</span><span>Projection v1</span></footer>
  </main>;
}
