export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export type ClosedTrade = {
  condition_id: string;
  asset: string;
  window: string;
  side: string | null;
  setup_id: string | null;
  book: string;
  action: string;
  outcome: "win" | "loss" | null;
  stake_micros: number | null;
  realized_pnl_micros: number;
  hold_secs: number | null;
  effective_cost_micros: number | null;
  ask_micros?: number | null;
  settlement_source: string | null;
  reason: string;
  observed_at_ns: number;
  stake_tier?: string | null;
  risk_bps?: number | null;
};

export type StrategyRow = {
  setup_id: string;
  book: string;
  totals: {
    closed_trades: number;
    wins: number;
    losses: number;
    realized_pnl_micros: number;
    open_positions: number;
    open_stake_micros: number;
    unrealized_pnl_micros: number;
    gross_win_micros?: number;
    gross_loss_micros?: number;
    best_trade_micros?: number;
    worst_trade_micros?: number;
    max_drawdown_micros?: number;
    current_streak?: number;
    best_win_streak?: number;
    worst_loss_streak?: number;
  };
  quality?: {
    win_rate_bps?: number | null;
    avg_win_micros?: number | null;
    avg_loss_micros?: number | null;
    expectancy_micros?: number | null;
    profit_factor_bps?: number | null;
    avg_hold_secs?: number | null;
  };
  win_rate_bps: number | null;
  total_pnl_micros: number;
};

export type PaperPayload = {
  live?: boolean;
  available?: boolean;
  halted?: boolean;
  status_age_ms?: number | null;
  tradable_bankroll_micros?: number | null;
  net_realized_micros?: number | null;
  withdraw_target_micros?: number | null;
  withdraw_progress_bps?: number | null;
  capital?: Record<string, number> | null;
  recovery?: Record<string, unknown> | null;
  settled_on_recovery?: number;
  status?: Record<string, unknown> | null;
  ledger?: {
    active_core?: Record<string, number>;
    shadow_experiment?: Record<string, number>;
    by_strategy?: StrategyRow[];
    by_asset?: [string, Record<string, number>][];
    by_window?: [string, Record<string, number>][];
    open_positions?: Record<string, unknown>[];
  };
  analytics?: {
    day_closes?: ClosedTrade[];
    recent_closed?: ClosedTrade[];
    recent_decisions?: Record<string, unknown>[];
    diagnostics?: Record<string, unknown>;
    windows?: Record<string, unknown>[];
    journal_day?: string;
    journal_decisions_loaded?: number;
  };
  portfolio?: Record<string, unknown>;
  [k: string]: unknown;
};

export type SuggestionAction = {
  action: string;
  setup_id?: string;
  window?: string;
  asset?: string;
  detail?: string;
  evidence?: string;
};

export type SuggestionRecord = {
  id: string;
  created_at: string;
  model: string;
  verdict: string;
  status: "new" | "accepted" | "rejected" | "implemented";
  actions: SuggestionAction[];
  raw?: string;
  note?: string;
  reject_reason?: string;
  spec_id?: string;
  commit?: string;
};

export type PatternRecord = {
  id: string;
  created_at: string;
  updated_at: string;
  title: string;
  hypothesis: string;
  status: "draft" | "shadow" | "active" | "killed";
  rules: string[];
  evidence: string;
  kill_criteria: string;
  linked_suggestion_id?: string;
  notes?: string;
};

export type ProposalInput = {
  actions: SuggestionAction[];
  raise_stake?: boolean;
  live_trading?: boolean;
};
