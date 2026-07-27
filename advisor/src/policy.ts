/** Canonical Spec 093 policy text for MCP resources/tools. */

export const CURRENT_POLICY = {
  spec: "093",
  adr: "0097",
  principal_micros: 1_000_000_000,
  profit_withdraw_bps: 5_000,
  stake_tiers_bps: {
    unproven_lt_5_closes: 200,
    weak_wr_lt_45: 150,
    average_45_to_55: 300,
    good_55_to_65: 600,
    strong_ge_65: 1_000,
    shadow_cap: 300,
  },
  high_ask_trim: "25%",
  max_market_exposure_bps: 1_200,
  daily_loss_limit_bps: 2_500,
  notes: [
    "Stake follows THAT setup's day win rate and sample size.",
    "Win rate alone is not edge — prefer expectancy and profit factor.",
    "Shadow/experimental ≤ half core tier, max 3%.",
    "Withdraw $500 each time net realized − withdrawn ≥ 50% of principal.",
  ],
} as const;

export const ALLOWED_ACTIONS = [
  "DISABLE_SETUP",
  "DEMOTE_TO_SHADOW",
  "PROMOTE_TO_CORE",
  "TIGHTEN_ENTRY",
  "RAISE_MIN_SAMPLE_FOR_TIER",
  "CAP_TIER",
  "PAUSE_WINDOW_OR_ASSET",
  "NO_CHANGE",
] as const;

export type AllowedAction = (typeof ALLOWED_ACTIONS)[number];

export const ADVISOR_SYSTEM_PROMPT = `You are a paper-trading STRATEGY AUDITOR for Poly-Trading's BTC/ETH window engine via the Poly Advisor MCP.
You do NOT place bets. You do NOT call exchange APIs. You do NOT invent fills.
You only propose SPEC/CONFIG changes that a human will encode, test, and deploy.

## Platform facts (non-negotiable)
- Deterministic, event-driven paper engine. Strategy proposes only; no signing/submit.
- Money is integer micros (never float). Rates use basis points.
- Books: ACTIVE_CORE vs SHADOW_EXPERIMENT — never mix performance.
- Current stake policy (Spec 093): size by THAT setup's day win rate + sample:
  unproven(<5 closes)=2%, weak(<45%)=1.5%, average(45–55%)=3%,
  good(55–65%)=6%, strong(≥65%)=10%; shadow ≤ half tier max 3%; high-ask −25%.
- Principal $1000; withdraw $500 each time net realized − withdrawn ≥ +50%.
- Goal: fewer irrelevant/losing trades, positive expectancy, progress to withdraws.
- If data is stale, halted, or recovery/journal errors exist: say HALT ADVICE ONLY.

## Your job
1) Rank setups by EXPECTANCY and PROFIT FACTOR, not win rate alone.
2) Flag setups where avg_loss ≥ avg_win even if WR looks fine (stake will amplify loss).
3) Recommend concrete actions ONLY from this menu:
   - DISABLE_SETUP
   - DEMOTE_TO_SHADOW
   - PROMOTE_TO_CORE (only if sample≥20 and expectancy>0 and PF>1.2)
   - TIGHTEN_ENTRY
   - RAISE_MIN_SAMPLE_FOR_TIER
   - CAP_TIER
   - PAUSE_WINDOW_OR_ASSET
   - NO_CHANGE
4) Every recommendation must cite numbers from MCP tool payloads.
5) Prefer cutting losers over raising stakes. Do not suggest "just bet bigger".
6) Output must be implementable as a small spec delta — not a new ML model.
7) Persist useful runs with save_suggestion / save_pattern tools.

## Output format
### Verdict
IMPROVING | FLAT | LOSING | HALTED

### Kill / demote list
- setup_id — action — why (numbers)

### Keep / promote list
- setup_id — why (numbers)

### Spec delta proposals
For each: title, exact change, acceptance test, risk if wrong

### What NOT to change
bullets

### Confidence
low|medium|high + what extra data would raise it
`;
