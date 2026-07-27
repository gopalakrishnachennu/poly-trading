import { readFile } from "node:fs/promises";
import { apiUrl, statusFile } from "./paths.js";
import type { PaperPayload } from "./types.js";

export async function fetchLivePayload(timeoutMs = 8_000): Promise<{
  ok: boolean;
  source: "api" | "status_file" | "none";
  error?: string;
  payload: PaperPayload | null;
}> {
  try {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), timeoutMs);
    const res = await fetch(apiUrl(), { signal: ctrl.signal });
    clearTimeout(t);
    if (!res.ok) {
      throw new Error(`HTTP ${res.status}`);
    }
    const payload = (await res.json()) as PaperPayload;
    return { ok: true, source: "api", payload };
  } catch (apiErr) {
    try {
      const raw = await readFile(statusFile(), "utf8");
      const status = JSON.parse(raw) as Record<string, unknown>;
      const payload: PaperPayload = {
        live: true,
        available: true,
        halted: Array.isArray(status.journal_errors) && status.journal_errors.length > 0,
        status,
        ledger: status.ledger as PaperPayload["ledger"],
        capital: status.capital as PaperPayload["capital"],
        tradable_bankroll_micros: status.tradable_bankroll_micros as number | undefined,
        net_realized_micros: status.net_realized_micros as number | undefined,
        withdraw_target_micros: status.withdraw_target_micros as number | undefined,
        withdraw_progress_bps: status.withdraw_progress_bps as number | undefined,
        recovery: status.recovery as PaperPayload["recovery"],
        analytics: undefined,
      };
      return {
        ok: true,
        source: "status_file",
        payload,
        error: `api unavailable: ${String(apiErr)}; fell back to status.json`,
      };
    } catch (fileErr) {
      return {
        ok: false,
        source: "none",
        payload: null,
        error: `api=${String(apiErr)}; file=${String(fileErr)}`,
      };
    }
  }
}

export function textResult(data: unknown): { content: { type: "text"; text: string }[] } {
  return {
    content: [
      {
        type: "text",
        text: typeof data === "string" ? data : JSON.stringify(data, null, 2),
      },
    ],
  };
}

export function errorResult(message: string): { content: { type: "text"; text: string }[]; isError: true } {
  return {
    content: [{ type: "text", text: message }],
    isError: true,
  };
}
