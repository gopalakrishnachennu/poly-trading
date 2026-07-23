# Edge Analysis — Complete-Set Arbitrage

**Question:** Does the complete-set arbitrage strategy the platform is built
around actually have a profitable edge in real captured market data?

**Short answer (this sample):** No. Across ~28,000 top-of-book observations
spanning ~5 hours of live BTC and ETH hourly markets, the complete-set pair
**never** offered a profitable arbitrage — not after fees, and not even at zero
fees. The market's bid/ask straddles the $1.00 fair value with a spread wide
enough to absorb any theoretical edge.

Reproduce with:

```bash
python3 scripts/analyze_paper_edge.py
```

## Method

One complete set (one Up + one Down share) redeems for exactly $1.00. Prices are
fixed-point micros (`1_000_000` = $1.00). For each observation:

- `buy_cost = up_best_ask + down_best_ask` — cost to buy the set now
- `sell_proceeds = up_best_bid + down_best_bid` — proceeds to sell a minted set
- A **buy** arb needs `buy_cost < $1.00`; a **sell** arb needs `sell_proceeds > $1.00`.
- Net of the policy reserves (fee $0.001 + slippage $0.0005 + min locked edge
  $0.001), the pair must clear **$0.0025 beyond $1.00** to count as an opportunity.

Data source: local paper-campaign JSONL journals under `var/paper-campaign/`
(Git-ignored capture from the read-only gateway).

## Results

| Metric | BTC | ETH |
| --- | --- | --- |
| Observations | 13,977 | 13,977 |
| `buy_cost` min / median / max | $1.000000 / $1.010000 / $1.060000 | $1.001000 / $1.010000 / $1.360000 |
| `sell_proceeds` min / median / max | $0.940000 / $0.990000 / $0.999000 | $0.640000 / $0.990000 / $0.999000 |
| Buy opportunities (net ≥ min edge) | 0 (0.0000%) | 0 (0.0000%) |
| Sell opportunities (net ≥ min edge) | 0 (0.0000%) | 0 (0.0000%) |
| Best buy net edge observed | −$0.001500 / share | −$0.002500 / share |
| Best sell net edge observed | −$0.002500 / share | −$0.002500 / share |

`buy_cost` distribution (whole cents above $1.00), both assets:

| Pair buy cost | Share of observations |
| --- | --- |
| $1.00–1.0099 | 10.1% |
| $1.01–1.0199 | 80.0% |
| $1.02–1.0299 | 8.5% |
| ≥ $1.03 | 1.4% |
| Exactly $1.00 | 2 of 27,954 observations |

## Interpretation

- **The edge is not eaten by fees — it is eaten by the bid/ask spread.** Even
  with zero fees, `buy_cost` is never below $1.00 and `sell_proceeds` is never
  above $1.00. The two sides of the pair sum to a ~1–2¢-wide band centered just
  above $1.00 on the ask and just below $1.00 on the bid. That is exactly what an
  efficient, market-made binary pair looks like.
- **The runtime is behaving correctly.** All paper campaigns produced
  `NO_TRADE` on every tick (`reason: "no conservative locked edge or feed
  quantity"`), and this analysis confirms that was the right call — there was
  nothing to capture. The safety stack is sound; the *signal* is absent.
- **A pure top-of-book taker of complete sets has no business here** in this
  regime. Any real complete-set arb on these venues would live in transient
  dislocations (news, resolution boundaries, liquidity gaps), require maker
  fills inside the spread, or require depth beyond top-of-book — none of which
  the current taker-at-top-of-book detector targets.

## Caveats

This is one ~5-hour capture of two assets at top-of-book only. It is strong
evidence that *taker complete-set arbitrage at top-of-book* is unprofitable in a
calm regime, but it is **not** proof that no edge exists anywhere. To generalize,
capture must cover more assets, many days, volatile and resolution windows, and
full order-book depth.

## Where an edge might actually live

99.9% of observations include both a `reference_price_micros` (the live Binance
index) and a `target_price_micros` (the hourly strike). The market resolves Up
if the reference is at/above the strike at the hour boundary. That is a
**directional / statistical** signal the current strategy does not use at all:
a fair Up-probability from distance-to-strike, time-to-expiry, and volatility can
be compared against the market mid. That comparison — not top-of-book
complete-set arb — is the more promising strategy surface to build and backtest
next.

## Recommended next steps

1. **Widen the capture** (more assets, multiple days, volatile + resolution
   windows) and re-run this tool to confirm the calm-regime result holds.
2. **Build a fair-value model** from `reference_price` vs `target_price` +
   time-to-expiry, and measure model-mid divergence against realized outcomes on
   the frozen paper datasets (a proper walk-forward backtest).
3. **Only then** decide whether the economically viable strategy is directional,
   maker-based, or dislocation-triggered — and point the (excellent) safety
   stack at whichever one the evidence supports.
