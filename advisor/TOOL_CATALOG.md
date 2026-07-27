# Poly Advisor — full MCP tool catalog (Spec 094)

61 tools + 7 resources. Research only — no place/cancel/sign.

## A — Data (17)
get_status, get_capital, get_health, get_open_book, get_setup_scoreboard,
get_setup_detail, get_rolls, get_day_diagnostics, get_recent_closes,
get_recent_decisions, get_live_tape, get_decisions_since, get_journal_slice,
get_day_archive, list_snapshots, get_snapshot, get_feature_snapshot

## B — Analytics (11)
rank_kill_keep, find_death_patterns, compare_days, streak_report,
exposure_report, stake_tier_audit, reject_reason_breakdown,
what_if_disable_setups, what_if_cap_tier, what_if_min_sample, edge_vs_size_report

## C — Patterns (6)
list_patterns, get_pattern, save_pattern, set_pattern_status, search_patterns,
pattern_performance

## D — Suggestions (6)
save_suggestion, list_suggestions, get_suggestion, accept_suggestion,
reject_suggestion, suggestion_diff_summary

## E — Specs (5)
get_current_policy, draft_spec_delta, validate_proposal_guards,
list_open_spec_work, mark_implemented

## F — Ops (6)
ops_overview, diagnose_halt, disk_usage, writer_lock_status, restart_playbook,
data_freshness

## G — Reports (5)
daily_briefing, trade_explain, loss_review, withdraw_progress_report,
export_day_report

## H — Guardrails (4)
assert_paper_only, forbid_live_order_tools, proposal_risk_score, require_citations

## Extra
capture_snapshot_now

## Resources
resource://poly/status
resource://poly/features
resource://poly/scoreboard
resource://poly/policy
resource://poly/advisor-prompt
resource://poly/patterns
resource://poly/suggestions/latest
