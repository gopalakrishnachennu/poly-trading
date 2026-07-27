"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

type ParamSchema = {
  name: string;
  type: "boolean" | "integer" | string;
  default?: boolean | number;
  minimum?: number;
  maximum?: number;
  unit?: string;
  description?: string;
  reload?: string;
  risk_warning?: string | null;
};

type PluginSchema = {
  plugin_type: string;
  setup_id: string;
  version: string;
  title: string;
  description?: string;
  paper_only: boolean;
  parameters: ParamSchema[];
};

type GateParams = Record<string, boolean | number>;

type ControlPayload = {
  paper_only: boolean;
  live_trading_authority: boolean;
  policy_root: string;
  setup_ids?: string[];
  plugins: PluginSchema[];
  schema: PluginSchema;
  defaults: {
    change_reason: string;
    setups: Record<string, GateParams>;
  };
  active: { version?: number; digest?: string; path?: string } | null;
  effective: {
    setups?: Record<string, GateParams>;
    change_reason?: string;
    version?: number;
  } | null;
  source: string;
  versions: {
    version: number;
    path: string;
    change_reason: string;
    setups: Record<string, GateParams>;
    late?: GateParams;
  }[];
  audit: Record<string, unknown>[];
};

function defaultsFromSchema(schema: PluginSchema): GateParams {
  const out: GateParams = {};
  for (const p of schema.parameters) {
    if (p.default !== undefined) out[p.name] = p.default;
  }
  return out;
}

export function StrategyBuilderPanel() {
  const [payload, setPayload] = useState<ControlPayload | null>(null);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [reason, setReason] = useState("gui_edit");
  const [setupId, setSetupId] = useState("5m_late_continuation");
  const [draft, setDraft] = useState<GateParams>({});
  const [bag, setBag] = useState<Record<string, GateParams>>({});
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);
  const [showJson, setShowJson] = useState(false);

  const plugins = payload?.plugins ?? [];
  const schema = plugins.find((p) => p.setup_id === setupId) ?? payload?.schema;
  const activeVersion = payload?.active?.version ?? null;

  const applySetup = useCallback(
    (id: string, setups: Record<string, GateParams>, pluginList: PluginSchema[]) => {
      const plugin = pluginList.find((p) => p.setup_id === id) ?? pluginList[0];
      if (!plugin) return;
      const base = defaultsFromSchema(plugin);
      const effective = setups[id] ?? {};
      setDraft({ ...base, ...effective });
      setSetupId(plugin.setup_id);
    },
    [],
  );

  const refresh = useCallback(async () => {
    try {
      const res = await fetch("/api/strategy-control", { cache: "no-store" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = (await res.json()) as ControlPayload;
      setPayload(data);
      setError("");
      const setups = data.effective?.setups ?? data.defaults.setups ?? {};
      setBag(setups);
      const preferred =
        data.plugins.find((p) => p.setup_id === setupId)?.setup_id
        ?? data.plugins[0]?.setup_id
        ?? "5m_late_continuation";
      applySetup(preferred, setups, data.plugins);
      setSelectedVersion(data.active?.version ?? data.versions[0]?.version ?? null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "strategy-control unavailable");
    }
  }, [applySetup, setupId]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 8_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const validationPreview = useMemo(() => {
    if (!schema) return [] as string[];
    const errors: string[] = [];
    const lo = Number(draft.entry_elapsed_lo_secs);
    const hi = Number(draft.entry_elapsed_hi_secs);
    if (hi < lo) errors.push("entry_elapsed_hi_secs must be >= entry_elapsed_lo_secs");
    const minC = Number(draft.min_confidence_micros);
    const maxC = Number(draft.max_confidence_micros ?? 0);
    if (maxC !== 0 && maxC < minC) {
      errors.push("max_confidence_micros must be >= min_confidence_micros");
    }
    return errors;
  }, [draft, schema]);

  async function post(action: string, extra: Record<string, unknown> = {}) {
    setBusy(true);
    setMessage("");
    try {
      const mergedSetups = { ...bag, [setupId]: draft };
      const res = await fetch("/api/strategy-control", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          action,
          change_reason: reason,
          setup_id: setupId,
          setup_params: draft,
          setups: mergedSetups,
          late_continuation: setupId === "5m_late_continuation" ? draft : undefined,
          ...extra,
        }),
      });
      const data = await res.json() as {
        ok?: boolean;
        errors?: string[];
        version?: number;
        result?: unknown;
        hint?: string;
      };
      if (!res.ok || data.ok === false) {
        setError((data.errors ?? [data.hint ?? "request failed"]).join("; "));
        return;
      }
      setError("");
      setMessage(
        action === "activate"
          ? `Activated version ${extra.version ?? selectedVersion}`
          : `Saved version ${data.version}`,
      );
      await refresh();
      if (data.version) setSelectedVersion(data.version);
    } catch (err) {
      setError(err instanceof Error ? err.message : "request failed");
    } finally {
      setBusy(false);
    }
  }

  function loadVersion(version: number) {
    const row = payload?.versions.find((v) => v.version === version);
    if (!row || !schema) return;
    const setups = row.setups ?? {};
    setBag(setups);
    applySetup(setupId, setups, plugins);
    setSelectedVersion(version);
    setReason(`edit_from_v${version}`);
    setMessage(`Loaded draft from v${version} (not activated)`);
  }

  function onSelectSetup(id: string) {
    const nextBag = { ...bag, [setupId]: draft };
    setBag(nextBag);
    applySetup(id, nextBag, plugins);
  }

  if (!payload || !schema) {
    return (
      <section className="strategy-builder" aria-label="Strategy builder">
        <div className="sb-head">
          <span className="panel-code">S1</span>
          <div>
            <strong>STRATEGY BUILDER</strong>
            <small>PAPER ONLY · SCHEMA-DRIVEN · SPEC 096 PHASE 4</small>
          </div>
        </div>
        <div className="sb-empty">{error || "Loading strategy-control…"}</div>
      </section>
    );
  }

  return (
    <section className="strategy-builder" aria-label="Strategy builder">
      <div className="sb-head">
        <span className="panel-code">S1</span>
        <div>
          <strong>STRATEGY BUILDER / {schema.title.toUpperCase()}</strong>
          <small>PAPER ONLY · NO LIVE AUTHORITY · ALL 8 SETUPS DIGEST-BOUND</small>
        </div>
        <b className={payload.source === "active_file" ? "positive" : "amber-text"}>
          {payload.source.toUpperCase()}
        </b>
      </div>

      <div className="sb-meta">
        <div>
          <span>PLUGIN</span>
          <label className="sb-select">
            <select
              value={setupId}
              onChange={(e) => onSelectSetup(e.target.value)}
              aria-label="Select setup plugin"
            >
              {plugins.map((p) => (
                <option key={p.setup_id} value={p.setup_id}>
                  {p.title}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div>
          <span>ACTIVE VERSION</span>
          <b>{activeVersion ?? "DEFAULTS"}</b>
        </div>
        <div>
          <span>DIGEST</span>
          <b>{payload.active?.digest ? `${payload.active.digest.slice(0, 12)}…` : "—"}</b>
        </div>
        <div>
          <span>RELOAD</span>
          <b>NEXT SIGNAL</b>
        </div>
        <div className="sb-env">
          <span>ENVIRONMENT</span>
          <b className="amber-text">PAPER</b>
        </div>
      </div>

      <div className="sb-grid">
        <div className="sb-form">
          <div className="section-label">
            PARAMETERS <span>{schema.setup_id}</span>
          </div>
          {schema.parameters.map((param) => {
            const value = draft[param.name];
            const unit = param.unit ? ` · ${param.unit}` : "";
            return (
              <label key={param.name} className="sb-field">
                <span>
                  {param.name}
                  {unit}
                  {param.reload ? <em>{param.reload}</em> : null}
                </span>
                {param.type === "boolean" ? (
                  <input
                    type="checkbox"
                    checked={Boolean(value)}
                    onChange={(e) => setDraft((d) => ({ ...d, [param.name]: e.target.checked }))}
                  />
                ) : (
                  <input
                    type="number"
                    value={Number(value ?? param.default ?? 0)}
                    min={param.minimum}
                    max={param.maximum}
                    step={1}
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, [param.name]: Number(e.target.value) }))
                    }
                  />
                )}
                <small>
                  {param.description}
                  {param.risk_warning ? ` ⚠ ${param.risk_warning}` : ""}
                </small>
              </label>
            );
          })}

          <label className="sb-field">
            <span>change_reason</span>
            <input value={reason} onChange={(e) => setReason(e.target.value)} />
            <small>Required for audit. Describes why this version exists.</small>
          </label>

          {validationPreview.length > 0 && (
            <div className="sb-errors">
              {validationPreview.map((e) => (
                <div key={e}>{e}</div>
              ))}
            </div>
          )}
          {error && <div className="sb-errors">{error}</div>}
          {message && <div className="sb-ok">{message}</div>}

          <div className="sb-actions">
            <button
              type="button"
              disabled={busy || validationPreview.length > 0}
              onClick={() => void post("validate")}
            >
              VALIDATE
            </button>
            <button
              type="button"
              disabled={busy || validationPreview.length > 0 || !reason.trim()}
              onClick={() => void post("save_draft")}
            >
              SAVE VERSION
            </button>
            <button
              type="button"
              className="sb-activate"
              disabled={busy || selectedVersion == null}
              onClick={() => {
                if (selectedVersion == null) return;
                if (!window.confirm(`Activate policy v${selectedVersion} for paper trading?`)) return;
                void post("activate", { version: selectedVersion });
              }}
            >
              ACTIVATE v{selectedVersion ?? "—"}
            </button>
            <button type="button" disabled={busy} onClick={() => void refresh()}>
              REFRESH
            </button>
            <button type="button" onClick={() => setShowJson((v) => !v)}>
              {showJson ? "HIDE JSON" : "RAW JSON"}
            </button>
          </div>
          {showJson && (
            <pre className="sb-json">
              {JSON.stringify(
                { setup_id: setupId, setup_params: draft, change_reason: reason },
                null,
                2,
              )}
            </pre>
          )}
        </div>

        <div className="sb-side">
          <div className="section-label">
            VERSIONS <span>{payload.versions.length} SAVED</span>
          </div>
          <div className="sb-versions">
            {payload.versions.length === 0 && <div className="sb-empty">No saved versions yet.</div>}
            {payload.versions.map((row) => (
              <button
                type="button"
                key={row.version}
                className={
                  selectedVersion === row.version
                    ? "sb-version active"
                    : activeVersion === row.version
                      ? "sb-version deployed"
                      : "sb-version"
                }
                onClick={() => loadVersion(row.version)}
              >
                <strong>v{row.version}</strong>
                <span>{row.change_reason}</span>
                <em>{activeVersion === row.version ? "ACTIVE" : "DRAFT"}</em>
              </button>
            ))}
          </div>
          <div className="sb-actions compact">
            <button
              type="button"
              disabled={busy || selectedVersion == null}
              onClick={() => void post("clone", { version: selectedVersion })}
            >
              CLONE SELECTED
            </button>
          </div>

          <div className="section-label" style={{ marginTop: 12 }}>
            AUDIT <span>LOCAL JSONL</span>
          </div>
          <div className="sb-audit">
            {payload.audit.length === 0 && <div className="sb-empty">No audit events yet.</div>}
            {payload.audit.slice(0, 12).map((ev, i) => (
              <div key={`${String(ev.at)}-${i}`}>
                <time>{String(ev.at ?? "").slice(11, 19) || "—"}</time>
                <b>{String(ev.event ?? "event")}</b>
                <span>
                  {ev.version != null ? `v${String(ev.version)}` : ""}{" "}
                  {ev.change_reason ? String(ev.change_reason) : ""}
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
