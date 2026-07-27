import { mkdir, readFile, readdir, writeFile, appendFile } from "node:fs/promises";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

export const dynamic = "force-dynamic";

/**
 * Spec 096 Phase 4 — Window Policy Control Plane API.
 * All eight setups are digest-bound; GUI edits one setup at a time and merges
 * into the full bag before save/activate.
 */

const REPO = join(process.cwd(), "..");
const POLICY_ROOT = process.env.POLY_STRATEGY_CONTROL_DIR
  ?? join(REPO, "var", "strategy-control");

type GateParams = Record<string, boolean | number>;

type PolicyDoc = {
  policy_id: string;
  version: number;
  paper_only: boolean;
  live_trading: boolean;
  change_reason: string;
  setups: Record<string, GateParams>;
};

const SETUP_IDS = [
  "5m_late_continuation",
  "5m_confirmed_mid_window_favourite",
  "5m_high_confidence_confirmation",
  "15m_four_minute_persistent_favourite",
  "15m_seven_minute_acceleration",
  "15m_eleven_minute_continuation",
  "1h_minute50_confirmed_favourite",
  "1h_experimental_sequential_hedge_seed",
] as const;

const MAX_ELAPSED: Record<string, number> = {
  "5m_late_continuation": 300,
  "5m_confirmed_mid_window_favourite": 300,
  "5m_high_confidence_confirmation": 300,
  "15m_four_minute_persistent_favourite": 900,
  "15m_seven_minute_acceleration": 900,
  "15m_eleven_minute_continuation": 900,
  "1h_minute50_confirmed_favourite": 3600,
  "1h_experimental_sequential_hedge_seed": 3600,
};

/** Compiled defaults mirrored from `SetupGateParams::*` in policy.rs. */
const DEFAULT_SETUPS: Record<string, GateParams> = {
  "5m_late_continuation": {
    enabled: true,
    entry_elapsed_lo_secs: 230,
    entry_elapsed_hi_secs: 245,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 600_000,
    max_confidence_micros: 820_000,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 820_000,
    min_persistence_secs: 0,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: false,
    require_positive_mom_med: true,
    require_non_negative_mom: false,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: false,
    shadow_ask_threshold_micros: 0,
  },
  "5m_confirmed_mid_window_favourite": {
    enabled: true,
    entry_elapsed_lo_secs: 135,
    entry_elapsed_hi_secs: 180,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 650_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 820_000,
    min_persistence_secs: 20,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: true,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: false,
    shadow_ask_threshold_micros: 0,
  },
  "5m_high_confidence_confirmation": {
    enabled: true,
    entry_elapsed_lo_secs: 150,
    entry_elapsed_hi_secs: 165,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 750_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 860_000,
    min_persistence_secs: 0,
    persistence_since_offset_secs: 120,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: true,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: true,
    shadow_ask_threshold_micros: 0,
  },
  "15m_four_minute_persistent_favourite": {
    enabled: true,
    entry_elapsed_lo_secs: 220,
    entry_elapsed_hi_secs: 260,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 600_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 840_000,
    min_persistence_secs: 30,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: true,
    require_positive_mom_med: false,
    require_non_negative_mom: false,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: false,
    shadow_ask_threshold_micros: 0,
  },
  "15m_seven_minute_acceleration": {
    enabled: true,
    entry_elapsed_lo_secs: 400,
    entry_elapsed_hi_secs: 440,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 600_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 840_000,
    min_persistence_secs: 60,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: false,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 50_000,
    require_reference_agrees: true,
    force_shadow_book: false,
    shadow_ask_threshold_micros: 0,
  },
  "15m_eleven_minute_continuation": {
    enabled: true,
    entry_elapsed_lo_secs: 640,
    entry_elapsed_hi_secs: 680,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 650_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 860_000,
    min_persistence_secs: 0,
    persistence_since_offset_secs: 600,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: false,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 20_000,
    require_reference_agrees: false,
    force_shadow_book: false,
    shadow_ask_threshold_micros: 800_000,
  },
  "1h_minute50_confirmed_favourite": {
    enabled: true,
    entry_elapsed_lo_secs: 2940,
    entry_elapsed_hi_secs: 3060,
    entry_elapsed_lo2_secs: 0,
    entry_elapsed_hi2_secs: 0,
    min_confidence_micros: 600_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 0,
    max_ask_override_micros: 880_000,
    min_persistence_secs: 300,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: false,
    require_mom_sign_agreement: true,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: true,
    shadow_ask_threshold_micros: 0,
  },
  "1h_experimental_sequential_hedge_seed": {
    enabled: true,
    entry_elapsed_lo_secs: 880,
    entry_elapsed_hi_secs: 920,
    entry_elapsed_lo2_secs: 2380,
    entry_elapsed_hi2_secs: 2420,
    min_confidence_micros: 600_000,
    max_confidence_micros: 0,
    min_confidence_window2_micros: 700_000,
    max_ask_override_micros: 880_000,
    min_persistence_secs: 0,
    persistence_since_offset_secs: 0,
    require_checkpoint_favourite: false,
    require_positive_mom_med: false,
    require_non_negative_mom: false,
    require_mom_sign_agreement: false,
    min_confidence_delta_60s_micros: 0,
    require_reference_agrees: false,
    force_shadow_book: true,
    shadow_ask_threshold_micros: 0,
  },
};

function gateSchema(setupId: string, title: string, description: string, defaults: GateParams) {
  const p = (name: string, type: string, description: string, extra: Record<string, unknown> = {}) => ({
    name,
    type,
    default: defaults[name],
    description,
    reload: "next_signal",
    ...extra,
  });
  return {
    plugin_type: "window_setup",
    setup_id: setupId,
    version: "2",
    title,
    description,
    paper_only: true,
    parameters: [
      p("enabled", "boolean", "When false, this setup never proposes entries."),
      p("entry_elapsed_lo_secs", "integer", "Earliest elapsed second for entry.", { unit: "seconds" }),
      p("entry_elapsed_hi_secs", "integer", "Latest elapsed second for entry.", { unit: "seconds" }),
      p("entry_elapsed_lo2_secs", "integer", "Optional second window start (0=unused).", { unit: "seconds" }),
      p("entry_elapsed_hi2_secs", "integer", "Optional second window end (0=unused).", { unit: "seconds" }),
      p("min_confidence_micros", "integer", "Minimum favourite confidence.", {
        unit: "micros", minimum: 0, maximum: 1_000_000,
        risk_warning: "Lowering this admits weaker signals.",
      }),
      p("max_confidence_micros", "integer", "Max confidence (0=no upper band).", {
        unit: "micros", minimum: 0, maximum: 1_000_000,
      }),
      p("min_confidence_window2_micros", "integer", "Min confidence in second window (0=use primary).", {
        unit: "micros",
      }),
      p("max_ask_override_micros", "integer", "Hard ask cap.", {
        unit: "micros", minimum: 0, maximum: 1_000_000,
        risk_warning: "Raising ask caps increases adverse selection risk.",
      }),
      p("min_persistence_secs", "integer", "Minimum favourite persistence.", { unit: "seconds" }),
      p("persistence_since_offset_secs", "integer", "Require persistence >= elapsed − offset (0=off).", {
        unit: "seconds",
      }),
      p("require_checkpoint_favourite", "boolean", "Require checkpoint favourite match."),
      p("require_positive_mom_med", "boolean", "Require medium momentum > 0."),
      p("require_non_negative_mom", "boolean", "Require non-negative medium/long momentum."),
      p("require_mom_sign_agreement", "boolean", "Require medium/long momentum sign agreement."),
      p("min_confidence_delta_60s_micros", "integer", "Minimum 60s confidence acceleration (0=off).", {
        unit: "micros",
      }),
      p("require_reference_agrees", "boolean", "Require reference agrees with favourite."),
      p("force_shadow_book", "boolean", "Always attribute to SHADOW_EXPERIMENT."),
      p("shadow_ask_threshold_micros", "integer", "Demote to shadow when ask above this (0=off).", {
        unit: "micros",
      }),
    ],
  };
}

const FALLBACK_PLUGINS = [
  gateSchema("5m_late_continuation", "5m late continuation", "Late-window favourite continuation.", DEFAULT_SETUPS["5m_late_continuation"]),
  gateSchema("5m_confirmed_mid_window_favourite", "5m mid-window favourite", "Confirmed mid-window favourite.", DEFAULT_SETUPS["5m_confirmed_mid_window_favourite"]),
  gateSchema("5m_high_confidence_confirmation", "5m high confidence", "High-confidence confirmation (shadow).", DEFAULT_SETUPS["5m_high_confidence_confirmation"]),
  gateSchema("15m_four_minute_persistent_favourite", "15m persistent favourite", "Four-minute persistent favourite.", DEFAULT_SETUPS["15m_four_minute_persistent_favourite"]),
  gateSchema("15m_seven_minute_acceleration", "15m acceleration", "Seven-minute acceleration.", DEFAULT_SETUPS["15m_seven_minute_acceleration"]),
  gateSchema("15m_eleven_minute_continuation", "15m continuation", "Eleven-minute continuation.", DEFAULT_SETUPS["15m_eleven_minute_continuation"]),
  gateSchema("1h_minute50_confirmed_favourite", "1h minute-50 favourite", "Minute-50 confirmed favourite (shadow).", DEFAULT_SETUPS["1h_minute50_confirmed_favourite"]),
  gateSchema("1h_experimental_sequential_hedge_seed", "1h sequential hedge seed", "Experimental sequential hedge seed (shadow).", DEFAULT_SETUPS["1h_experimental_sequential_hedge_seed"]),
];

function defaultDoc(version = 0, reason = "compiled_defaults"): PolicyDoc {
  const setups: Record<string, GateParams> = {};
  for (const id of SETUP_IDS) {
    setups[id] = { ...DEFAULT_SETUPS[id] };
  }
  return {
    policy_id: "window_paper_policy",
    version,
    paper_only: true,
    live_trading: false,
    change_reason: reason,
    setups,
  };
}

function normalizeSetups(raw: Record<string, GateParams> | undefined): Record<string, GateParams> {
  const out: Record<string, GateParams> = {};
  for (const id of SETUP_IDS) {
    out[id] = { ...DEFAULT_SETUPS[id], ...(raw?.[id] ?? {}) };
  }
  return out;
}

function validateGate(setupId: string, p: GateParams): string[] {
  const errors: string[] = [];
  const tag = (msg: string) => `${setupId}: ${msg}`;
  const lo = Number(p.entry_elapsed_lo_secs);
  const hi = Number(p.entry_elapsed_hi_secs);
  const maxElapsed = MAX_ELAPSED[setupId] ?? 3600;
  if (hi < lo) errors.push(tag("entry_elapsed_hi_secs must be >= entry_elapsed_lo_secs"));
  if (lo < 0 || hi > maxElapsed) {
    errors.push(tag(`entry window must be within 0..=${maxElapsed}`));
  }
  const lo2 = Number(p.entry_elapsed_lo2_secs ?? 0);
  const hi2 = Number(p.entry_elapsed_hi2_secs ?? 0);
  if (lo2 !== 0 || hi2 !== 0) {
    if (hi2 < lo2) errors.push(tag("entry_elapsed_hi2_secs must be >= entry_elapsed_lo2_secs"));
    if (lo2 < 0 || hi2 > maxElapsed) {
      errors.push(tag(`second entry window must be within 0..=${maxElapsed}`));
    }
  }
  const minC = Number(p.min_confidence_micros);
  const maxC = Number(p.max_confidence_micros ?? 0);
  if (minC < 0 || minC > 1_000_000) {
    errors.push(tag("min_confidence_micros must be within 0..=1_000_000"));
  }
  if (maxC !== 0) {
    if (maxC > 1_000_000) errors.push(tag("max_confidence_micros must be <= 1_000_000"));
    if (maxC < minC) errors.push(tag("max_confidence_micros must be >= min_confidence_micros"));
  }
  const ask = Number(p.max_ask_override_micros);
  if (ask < 0 || ask > 1_000_000) {
    errors.push(tag("max_ask_override_micros must be within 0..=1_000_000"));
  }
  return errors;
}

function validate(doc: PolicyDoc): string[] {
  const errors: string[] = [];
  if (!doc.paper_only) errors.push("paper_only must be true");
  if (doc.live_trading) errors.push("live_trading must be false");
  for (const id of SETUP_IDS) {
    const p = doc.setups?.[id];
    if (!p) {
      errors.push(`missing setups.${id}`);
      continue;
    }
    errors.push(...validateGate(id, p));
  }
  return errors;
}

async function ensureDirs() {
  await mkdir(join(POLICY_ROOT, "versions"), { recursive: true });
}

async function nextVersion(): Promise<number> {
  try {
    const files = await readdir(join(POLICY_ROOT, "versions"));
    let max = 0;
    for (const f of files) {
      const m = /^v(\d+)\.json$/.exec(f);
      if (m) max = Math.max(max, Number(m[1]));
    }
    return max + 1;
  } catch {
    return 1;
  }
}

async function listVersions(): Promise<{
  version: number;
  path: string;
  change_reason: string;
  setups: Record<string, GateParams>;
}[]> {
  try {
    const files = (await readdir(join(POLICY_ROOT, "versions")))
      .filter((f) => /^v\d+\.json$/.test(f))
      .sort()
      .reverse();
    const out = [];
    for (const f of files.slice(0, 50)) {
      const doc = JSON.parse(await readFile(join(POLICY_ROOT, "versions", f), "utf8")) as PolicyDoc;
      out.push({
        version: doc.version,
        path: `versions/${f}`,
        change_reason: doc.change_reason,
        setups: normalizeSetups(doc.setups),
      });
    }
    return out;
  } catch {
    return [];
  }
}

async function readAudit(limit = 40): Promise<unknown[]> {
  try {
    const raw = await readFile(join(POLICY_ROOT, "audit.jsonl"), "utf8");
    return raw
      .split("\n")
      .filter(Boolean)
      .slice(-limit)
      .reverse()
      .map((line) => {
        try {
          return JSON.parse(line);
        } catch {
          return { raw: line };
        }
      });
  } catch {
    return [];
  }
}

async function appendAudit(event: Record<string, unknown>) {
  await ensureDirs();
  await appendFile(
    join(POLICY_ROOT, "audit.jsonl"),
    `${JSON.stringify({ ...event, at: new Date().toISOString() })}\n`,
    "utf8",
  );
}

async function readActive() {
  try {
    const raw = await readFile(join(POLICY_ROOT, "active.json"), "utf8");
    const pointer = JSON.parse(raw) as { path?: string; version?: number; digest?: string };
    const path = pointer.path
      ? (pointer.path.startsWith("/") ? pointer.path : join(POLICY_ROOT, pointer.path))
      : null;
    if (!path) return { pointer, document: null, note: "active_file" };
    const doc = JSON.parse(await readFile(path, "utf8")) as PolicyDoc;
    return {
      pointer,
      document: { ...doc, setups: normalizeSetups(doc.setups) },
      note: "active_file",
    };
  } catch {
    return { pointer: null, document: defaultDoc(), note: "compiled_defaults" };
  }
}

function runPolicyCli(args: string[]): { ok: boolean; stdout: string; stderr: string } {
  const bin = join(REPO, "target/release/btc-window-policy");
  const result = spawnSync(bin, args, {
    cwd: REPO,
    env: { ...process.env, POLY_STRATEGY_CONTROL_DIR: POLICY_ROOT },
    encoding: "utf8",
  });
  return {
    ok: result.status === 0,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function loadPlugins() {
  const cli = runPolicyCli(["plugins"]);
  if (cli.ok) {
    try {
      const parsed = JSON.parse(cli.stdout);
      if (Array.isArray(parsed) && parsed.length >= 8) return parsed;
    } catch {
      /* fall through */
    }
  }
  return FALLBACK_PLUGINS;
}

function mergeDocument(
  base: PolicyDoc,
  opts: {
    setupId?: string;
    setupParams?: GateParams;
    late_continuation?: GateParams;
    setups?: Record<string, GateParams>;
    document?: PolicyDoc;
    change_reason?: string;
  },
): PolicyDoc {
  const next: PolicyDoc = {
    ...base,
    setups: normalizeSetups(base.setups),
  };
  if (opts.document?.setups) {
    next.setups = normalizeSetups(opts.document.setups);
    next.change_reason = opts.document.change_reason || next.change_reason;
  }
  if (opts.setups) {
    next.setups = normalizeSetups({ ...next.setups, ...opts.setups });
  }
  // Backward-compatible Phase-3 field.
  if (opts.late_continuation) {
    next.setups["5m_late_continuation"] = {
      ...next.setups["5m_late_continuation"],
      ...opts.late_continuation,
    };
  }
  if (opts.setupId && opts.setupParams) {
    next.setups[opts.setupId] = {
      ...next.setups[opts.setupId],
      ...opts.setupParams,
    };
  }
  if (opts.change_reason) next.change_reason = opts.change_reason;
  next.paper_only = true;
  next.live_trading = false;
  return next;
}

export async function GET() {
  const active = await readActive();
  const versions = await listVersions();
  const audit = await readAudit();
  const plugins = loadPlugins();
  const schema = plugins[0];
  return Response.json({
    paper_only: true,
    live_trading_authority: false,
    policy_root: POLICY_ROOT,
    setup_ids: SETUP_IDS,
    plugins,
    schema,
    defaults: defaultDoc(),
    active: active.pointer,
    effective: active.document,
    source: active.note,
    versions,
    audit,
  });
}

export async function POST(req: Request) {
  const body = await req.json() as {
    action?: string;
    document?: PolicyDoc;
    version?: number;
    change_reason?: string;
    setup_id?: string;
    setup_params?: GateParams;
    late_continuation?: GateParams;
    setups?: Record<string, GateParams>;
  };
  await ensureDirs();

  if (body.action === "validate") {
    const doc = mergeDocument(defaultDoc(), body);
    const errors = validate(doc);
    return Response.json({ ok: errors.length === 0, errors, document: doc });
  }

  if (body.action === "save_draft" || body.action === "save_version" || body.action === "clone") {
    let base = mergeDocument(defaultDoc(), body);
    if (body.action === "clone" && body.version) {
      const rel = `versions/v${String(body.version).padStart(4, "0")}.json`;
      try {
        const prior = JSON.parse(await readFile(join(POLICY_ROOT, rel), "utf8")) as PolicyDoc;
        base = mergeDocument(prior, {
          setupId: body.setup_id,
          setupParams: body.setup_params,
          late_continuation: body.late_continuation,
          setups: body.setups,
          change_reason: body.change_reason ?? `clone_of_v${body.version}`,
        });
      } catch {
        return Response.json({ ok: false, errors: [`missing ${rel}`] }, { status: 404 });
      }
    } else {
      base.change_reason = body.change_reason ?? base.change_reason ?? "api_save";
    }
    base.version = await nextVersion();
    const errors = validate(base);
    if (errors.length) {
      return Response.json({ ok: false, errors }, { status: 400 });
    }
    const rel = `versions/v${String(base.version).padStart(4, "0")}.json`;
    await writeFile(join(POLICY_ROOT, rel), `${JSON.stringify(base, null, 2)}\n`, "utf8");
    await appendAudit({
      event: body.action === "clone" ? "clone_version" : "save_version",
      version: base.version,
      change_reason: base.change_reason,
      source: "terminal_api",
      setup_id: body.setup_id ?? null,
    });
    return Response.json({ ok: true, version: base.version, path: rel, document: base });
  }

  if (body.action === "activate") {
    const version = body.version;
    if (!version || version < 1) {
      return Response.json({ ok: false, errors: ["version required"] }, { status: 400 });
    }
    const cli = runPolicyCli(["activate", String(version)]);
    if (!cli.ok) {
      return Response.json({
        ok: false,
        errors: [cli.stderr.trim() || "btc-window-policy activate failed — build release binary"],
        hint: "cargo build --release -p btc-window-strategy --bin btc-window-policy",
      }, { status: 500 });
    }
    await appendAudit({ event: "activate_request", version, source: "terminal_api" });
    try {
      return Response.json({ ok: true, result: JSON.parse(cli.stdout) });
    } catch {
      return Response.json({ ok: true, raw: cli.stdout });
    }
  }

  return Response.json({
    ok: false,
    errors: ["unknown action — use validate | save_draft | clone | activate"],
  }, { status: 400 });
}
