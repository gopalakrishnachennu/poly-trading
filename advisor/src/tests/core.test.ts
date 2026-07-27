import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { avgMicros, expectancyMicros, profitFactorBps, winRateBps } from "../money.js";
import { whatIfDisableSetups, whatIfMinSample } from "../whatif.js";
import { validateProposal, requireCitations } from "../validate.js";
import { savePattern, saveSuggestion, getSuggestion, listPatterns } from "../store.js";
import type { ClosedTrade } from "../types.js";

test("money helpers stay integer", () => {
  assert.equal(avgMicros(10_000_005, 3), 3_333_335);
  assert.equal(winRateBps(2, 5), 4_000);
  assert.equal(profitFactorBps(20_000, 10_000), 20_000);
  const exp = expectancyMicros(2, 2, 10_000, 8_000);
  assert.equal(exp, Math.trunc((2 * 5_000 - 2 * 4_000) / 4));
});

test("what-if disable removes setup pnl", () => {
  const closes: ClosedTrade[] = [
    {
      condition_id: "a",
      asset: "BTC",
      window: "5m",
      side: "up",
      setup_id: "bad",
      book: "ACTIVE_CORE",
      action: "settle",
      outcome: "loss",
      stake_micros: 1_000_000,
      realized_pnl_micros: -5_000_000,
      hold_secs: 10,
      effective_cost_micros: null,
      settlement_source: null,
      reason: "x",
      observed_at_ns: 1,
    },
    {
      condition_id: "b",
      asset: "BTC",
      window: "5m",
      side: "up",
      setup_id: "good",
      book: "ACTIVE_CORE",
      action: "settle",
      outcome: "win",
      stake_micros: 1_000_000,
      realized_pnl_micros: 3_000_000,
      hold_secs: 10,
      effective_cost_micros: null,
      settlement_source: null,
      reason: "x",
      observed_at_ns: 2,
    },
  ];
  const r = whatIfDisableSetups(closes, ["bad"]);
  assert.equal(r.after.realized_pnl_micros, 3_000_000);
  assert.equal(r.delta_realized_pnl_micros, 5_000_000);
});

test("what-if min sample skips early trades", () => {
  const closes: ClosedTrade[] = [];
  for (let i = 0; i < 5; i++) {
    closes.push({
      condition_id: `c${i}`,
      asset: "ETH",
      window: "15m",
      side: "down",
      setup_id: "s",
      book: "ACTIVE_CORE",
      action: "settle",
      outcome: i % 2 === 0 ? "win" : "loss",
      stake_micros: 1_000_000,
      realized_pnl_micros: i % 2 === 0 ? 1_000_000 : -1_000_000,
      hold_secs: 5,
      effective_cost_micros: null,
      settlement_source: null,
      reason: "x",
      observed_at_ns: i + 1,
    });
  }
  const r = whatIfMinSample(closes, 3);
  assert.equal(r.skipped.sample_size, 3);
  assert.equal(r.after.sample_size, 2);
});

test("proposal guards reject live trading and unknown actions", () => {
  const bad = validateProposal({
    live_trading: true,
    actions: [{ action: "PLACE_ORDER", evidence: "100" }],
  });
  assert.equal(bad.ok, false);
  assert.ok(bad.errors.some((e) => e.includes("live_trading")));
  assert.ok(bad.errors.some((e) => e.includes("not in menu")));

  const good = validateProposal({
    actions: [{ action: "DISABLE_SETUP", setup_id: "x", evidence: "expectancy -2.5 on 12 closes" }],
  });
  assert.equal(good.ok, true);
});

test("require citations needs digits", () => {
  const r = requireCitations([{ action: "NO_CHANGE", evidence: "feels bad" }]);
  assert.equal(r.ok, false);
  const r2 = requireCitations([{ action: "NO_CHANGE", evidence: "expectancy -100000 on 9 closes" }]);
  assert.equal(r2.ok, true);
});

test("suggestion and pattern store round-trip", async () => {
  const dir = await mkdtemp(join(tmpdir(), "poly-advisor-"));
  process.env.POLY_ADVISOR_ROOT = dir;
  try {
    const sug = await saveSuggestion({
      model: "test",
      verdict: "LOSING",
      actions: [{ action: "DISABLE_SETUP", setup_id: "x", evidence: "exp -1 on 10" }],
    });
    const got = await getSuggestion(sug.id);
    assert.equal(got?.verdict, "LOSING");
    const pat = await savePattern({
      title: "Cut fat losers",
      hypothesis: "high WR fat loss setups destroy bankroll",
      rules: ["setup_id: x"],
      evidence: "PF < 1",
      kill_criteria: "expectancy > 0 for 20 closes",
    });
    const listed = await listPatterns();
    assert.ok(listed.some((p) => p.id === pat.id));
  } finally {
    await rm(dir, { recursive: true, force: true });
    delete process.env.POLY_ADVISOR_ROOT;
  }
});
