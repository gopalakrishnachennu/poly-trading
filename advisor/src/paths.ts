import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));

/** Repo root containing `advisor/` and `var/`. */
export function repoRoot(): string {
  const env = process.env.POLY_REPO_ROOT;
  if (env) return resolve(env);
  // src/ or dist/ → advisor/ → repo/
  return resolve(here, "../..");
}

export function advisorRoot(): string {
  const env = process.env.POLY_ADVISOR_ROOT;
  if (env) return resolve(env);
  return join(repoRoot(), "var", "poly-advisor");
}

export function paperDir(): string {
  const env = process.env.POLY_BTC_WINDOW_PAPER_DIR;
  if (env) return resolve(env);
  return join(repoRoot(), "var", "btc-window-paper");
}

export function apiUrl(): string {
  return process.env.POLY_ADVISOR_API_URL
    ?? "http://127.0.0.1:3000/api/btc-window-paper";
}

export function statusFile(): string {
  return join(paperDir(), "status.json");
}

export function journalPathForDay(day: string): string {
  return join(paperDir(), `decisions-${day}.jsonl`);
}

export function todayUtc(): string {
  return new Date().toISOString().slice(0, 10);
}

export async function ensureAdvisorDirs(): Promise<void> {
  const root = advisorRoot();
  for (const sub of [
    "snapshots",
    "features",
    "tape",
    "suggestions",
    "patterns",
    "reports",
    "prompts",
    "archives",
  ]) {
    await mkdir(join(root, sub), { recursive: true });
  }
}

export const paths = {
  snapshots: (day = todayUtc()) => join(advisorRoot(), "snapshots", day),
  featuresDir: () => join(advisorRoot(), "features"),
  featuresLatest: () => join(advisorRoot(), "features", "latest.json"),
  tapeLatest: () => join(advisorRoot(), "tape", "latest.jsonl"),
  tapeMeta: () => join(advisorRoot(), "tape", "meta.json"),
  suggestionsDir: () => join(advisorRoot(), "suggestions"),
  patternsDir: () => join(advisorRoot(), "patterns"),
  reportsDir: () => join(advisorRoot(), "reports"),
  promptsDir: () => join(advisorRoot(), "prompts"),
  archivesDir: () => join(advisorRoot(), "archives"),
  advisorPrompt: () => join(advisorRoot(), "prompts", "advisor-system.md"),
};
