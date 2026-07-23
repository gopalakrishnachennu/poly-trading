import { readFile, readdir, stat } from "node:fs/promises";
import { join } from "node:path";

// Inventory of everything this system has saved to disk, plus the backtest
// readiness summary written by `scripts/capture_progress.py --json`.
//
// The directory scan is deliberately cheap (file counts, bytes, mtimes) so this
// stays responsive against multi-gigabyte capture journals; record-level counts
// come from the precomputed summary rather than parsing on every request.
export const dynamic = "force-dynamic";

type Dataset = {
  key: string;
  label: string;
  path: string;
  what: string;
  files: number;
  bytes: number;
  newest_ms: number | null;
};

const DATASETS: { key: string; label: string; dir: string; what: string }[] = [
  { key: "research", label: "RESEARCH CAPTURE", dir: "var/research-capture", what: "Compact market snapshots (prices + book sizes). Feeds the engine." },
  { key: "campaign", label: "PAPER CAMPAIGNS", dir: "var/paper-campaign", what: "Rust paper-campaign journals: observations, decisions, checkpoints." },
  { key: "tick", label: "RAW TICK CAPTURE", dir: "var/tick-capture", what: "Full CLOB + reference event journals. Authoritative, large." },
  { key: "live", label: "LIVE PAPER BOOKS", dir: "var/live-paper", what: "Live trader bankrolls, open positions and settled bets." },
  { key: "export", label: "RESEARCH EXPORT", dir: "var/research-export", what: "Derived CSV/Parquet partitions by asset/date/hour." },
  { key: "logs", label: "LOGS", dir: "var/log", what: "Gateway, recorder and trader logs." },
];

async function walk(dir: string): Promise<{ files: number; bytes: number; newest: number | null }> {
  let files = 0;
  let bytes = 0;
  let newest: number | null = null;
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return { files, bytes, newest };
  }
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      const sub = await walk(full);
      files += sub.files;
      bytes += sub.bytes;
      if (sub.newest !== null) newest = Math.max(newest ?? 0, sub.newest);
      continue;
    }
    try {
      const info = await stat(full);
      files += 1;
      bytes += info.size;
      newest = Math.max(newest ?? 0, info.mtimeMs);
    } catch { /* file vanished mid-scan */ }
  }
  return { files, bytes, newest };
}

export async function GET() {
  const root = join(process.cwd(), "..");
  const datasets: Dataset[] = [];
  for (const d of DATASETS) {
    const { files, bytes, newest } = await walk(join(root, d.dir));
    datasets.push({ key: d.key, label: d.label, path: d.dir, what: d.what, files, bytes, newest_ms: newest ? Math.round(newest) : null });
  }

  let summary: unknown = null;
  try {
    summary = JSON.parse(await readFile(join(root, "var", "data-summary.json"), "utf8"));
  } catch { /* not generated yet */ }

  return Response.json({
    available: true,
    generated_at_ms: Date.now(),
    datasets,
    summary,
    summary_hint: "refresh with: make data-summary",
  });
}
