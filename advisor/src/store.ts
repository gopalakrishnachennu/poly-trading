import { randomUUID } from "node:crypto";
import { readdir, readFile, writeFile, mkdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { ensureAdvisorDirs, paths } from "./paths.js";
import type { PatternRecord, SuggestionRecord } from "./types.js";

async function writeJson(path: string, data: unknown): Promise<void> {
  await writeFile(path, `${JSON.stringify(data, null, 2)}\n`, "utf8");
}

export async function saveSuggestion(
  input: Omit<SuggestionRecord, "id" | "created_at" | "status"> & { status?: SuggestionRecord["status"] },
): Promise<SuggestionRecord> {
  await ensureAdvisorDirs();
  const rec: SuggestionRecord = {
    id: `sug_${randomUUID().slice(0, 8)}`,
    created_at: new Date().toISOString(),
    status: input.status ?? "new",
    model: input.model,
    verdict: input.verdict,
    actions: input.actions,
    raw: input.raw,
    note: input.note,
  };
  await writeJson(join(paths.suggestionsDir(), `${rec.id}.json`), rec);
  await writeJson(join(paths.suggestionsDir(), "latest.json"), rec);
  return rec;
}

export async function listSuggestions(limit = 50, status?: string): Promise<SuggestionRecord[]> {
  await ensureAdvisorDirs();
  const files = (await readdir(paths.suggestionsDir())).filter((f) => f.startsWith("sug_") && f.endsWith(".json"));
  const rows: SuggestionRecord[] = [];
  for (const f of files) {
    const raw = await readFile(join(paths.suggestionsDir(), f), "utf8");
    const rec = JSON.parse(raw) as SuggestionRecord;
    if (status && rec.status !== status) continue;
    rows.push(rec);
  }
  rows.sort((a, b) => b.created_at.localeCompare(a.created_at));
  return rows.slice(0, limit);
}

export async function getSuggestion(id: string): Promise<SuggestionRecord | null> {
  try {
    const raw = await readFile(join(paths.suggestionsDir(), `${id}.json`), "utf8");
    return JSON.parse(raw) as SuggestionRecord;
  } catch {
    return null;
  }
}

export async function updateSuggestion(
  id: string,
  patch: Partial<SuggestionRecord>,
): Promise<SuggestionRecord | null> {
  const cur = await getSuggestion(id);
  if (!cur) return null;
  const next = { ...cur, ...patch, id: cur.id, created_at: cur.created_at };
  await writeJson(join(paths.suggestionsDir(), `${id}.json`), next);
  await writeJson(join(paths.suggestionsDir(), "latest.json"), next);
  return next;
}

export async function savePattern(
  input: Omit<PatternRecord, "id" | "created_at" | "updated_at" | "status"> & {
    id?: string;
    status?: PatternRecord["status"];
  },
): Promise<PatternRecord> {
  await ensureAdvisorDirs();
  const now = new Date().toISOString();
  const id = input.id ?? `pat_${randomUUID().slice(0, 8)}`;
  let created_at = now;
  try {
    const existing = await readFile(join(paths.patternsDir(), `${id}.json`), "utf8");
    created_at = (JSON.parse(existing) as PatternRecord).created_at;
  } catch {
    /* new */
  }
  const rec: PatternRecord = {
    id,
    created_at,
    updated_at: now,
    title: input.title,
    hypothesis: input.hypothesis,
    status: input.status ?? "draft",
    rules: input.rules,
    evidence: input.evidence,
    kill_criteria: input.kill_criteria,
    linked_suggestion_id: input.linked_suggestion_id,
    notes: input.notes,
  };
  await writeJson(join(paths.patternsDir(), `${rec.id}.json`), rec);
  return rec;
}

export async function listPatterns(status?: string): Promise<PatternRecord[]> {
  await ensureAdvisorDirs();
  const files = (await readdir(paths.patternsDir())).filter((f) => f.startsWith("pat_") && f.endsWith(".json"));
  const rows: PatternRecord[] = [];
  for (const f of files) {
    const rec = JSON.parse(await readFile(join(paths.patternsDir(), f), "utf8")) as PatternRecord;
    if (status && rec.status !== status) continue;
    rows.push(rec);
  }
  rows.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  return rows;
}

export async function getPattern(id: string): Promise<PatternRecord | null> {
  try {
    return JSON.parse(await readFile(join(paths.patternsDir(), `${id}.json`), "utf8")) as PatternRecord;
  } catch {
    return null;
  }
}

export async function setPatternStatus(
  id: string,
  status: PatternRecord["status"],
): Promise<PatternRecord | null> {
  const cur = await getPattern(id);
  if (!cur) return null;
  return savePattern({ ...cur, status });
}

export async function searchPatterns(query: string): Promise<PatternRecord[]> {
  const q = query.toLowerCase();
  const all = await listPatterns();
  return all.filter((p) =>
    [p.title, p.hypothesis, p.evidence, p.kill_criteria, ...p.rules].join("\n").toLowerCase().includes(q)
  );
}

export async function exportReport(name: string, body: string, format: "md" | "json"): Promise<string> {
  await ensureAdvisorDirs();
  const file = join(paths.reportsDir(), `${name}.${format}`);
  await writeFile(file, body.endsWith("\n") ? body : `${body}\n`, "utf8");
  return file;
}

export async function dirSizeBytes(dir: string): Promise<number> {
  try {
    const entries = await readdir(dir, { withFileTypes: true });
    let total = 0;
    for (const e of entries) {
      const p = join(dir, e.name);
      if (e.isDirectory()) total += await dirSizeBytes(p);
      else total += (await stat(p)).size;
    }
    return total;
  } catch {
    return 0;
  }
}

export async function appendTape(decision: Record<string, unknown>): Promise<void> {
  await ensureAdvisorDirs();
  const { appendFile } = await import("node:fs/promises");
  await appendFile(paths.tapeLatest(), `${JSON.stringify(decision)}\n`, "utf8");
}

export async function rotateTapeIfHuge(maxBytes = 8_000_000): Promise<void> {
  try {
    const s = await stat(paths.tapeLatest());
    if (s.size <= maxBytes) return;
    const { rename } = await import("node:fs/promises");
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    await rename(paths.tapeLatest(), join(paths.tapeLatest(), "..", `tape-${stamp}.jsonl`));
  } catch {
    /* empty */
  }
}

// silence unused mkdir import if tree-shaken oddly
void mkdir;
