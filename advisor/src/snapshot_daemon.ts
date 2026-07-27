/**
 * 1 Hz snapshot daemon for Poly Advisor (Spec 094).
 * Writes var/poly-advisor/snapshots + features/latest.json.
 */
import { writeSnapshotOnce } from "./features.js";

const intervalMs = Number(process.env.POLY_ADVISOR_SNAPSHOT_MS ?? 1000);

async function tick() {
  const r = await writeSnapshotOnce();
  const ts = new Date().toISOString();
  if (r.ok) {
    console.error(`[poly-advisor-snapshot] ${ts} ok ${r.path}`);
  } else {
    console.error(`[poly-advisor-snapshot] ${ts} fail ${r.error}`);
  }
}

console.error(`[poly-advisor-snapshot] starting interval_ms=${intervalMs}`);
await tick();
setInterval(() => {
  void tick();
}, intervalMs);
