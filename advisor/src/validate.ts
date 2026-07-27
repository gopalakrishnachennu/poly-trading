import { ALLOWED_ACTIONS, type AllowedAction } from "./policy.js";
import type { ProposalInput, SuggestionAction } from "./types.js";

export function validateProposal(proposal: ProposalInput): {
  ok: boolean;
  errors: string[];
  warnings: string[];
  risk: "low" | "medium" | "high";
} {
  const errors: string[] = [];
  const warnings: string[] = [];

  if (proposal.live_trading) {
    errors.push("live_trading is forbidden — MCP is paper research only (ADR 0098)");
  }
  if (proposal.raise_stake) {
    warnings.push("Raising stake without positive expectancy usually worsens losses — prefer CAP_TIER / DISABLE");
  }
  if (!proposal.actions?.length) {
    errors.push("actions array is empty");
  }
  for (const a of proposal.actions ?? []) {
    if (!ALLOWED_ACTIONS.includes(a.action as AllowedAction)) {
      errors.push(`action not in menu: ${a.action}`);
    }
    if (!a.evidence || a.evidence.trim().length < 8) {
      warnings.push(`action ${a.action} missing strong numeric evidence citation`);
    }
    if (a.action === "PROMOTE_TO_CORE" && !String(a.evidence ?? "").includes("expectancy")) {
      warnings.push("PROMOTE_TO_CORE should cite expectancy and sample size");
    }
  }

  let risk: "low" | "medium" | "high" = "low";
  if (proposal.raise_stake) risk = "high";
  if (proposal.actions?.some((a) => a.action === "PROMOTE_TO_CORE")) risk = risk === "high" ? "high" : "medium";
  if (errors.length) risk = "high";

  return { ok: errors.length === 0, errors, warnings, risk };
}

export function draftSpecDelta(actions: SuggestionAction[], suggestionId?: string): string {
  const lines = [
    `# Draft Spec Delta (from Poly Advisor)`,
    ``,
    `Source suggestion: ${suggestionId ?? "n/a"}`,
    `Related: Spec 093 / ADR 0097 stake policy; Spec 094 advisor MCP.`,
    ``,
    `## Objective`,
    `Apply advisor-accepted paper research actions without granting live authority.`,
    ``,
    `## Proposed changes`,
  ];
  for (const [i, a] of actions.entries()) {
    lines.push(`${i + 1}. **${a.action}**` + (a.setup_id ? ` — \`${a.setup_id}\`` : ""));
    if (a.window) lines.push(`   - window: ${a.window}`);
    if (a.asset) lines.push(`   - asset: ${a.asset}`);
    if (a.detail) lines.push(`   - detail: ${a.detail}`);
    if (a.evidence) lines.push(`   - evidence: ${a.evidence}`);
  }
  lines.push(
    ``,
    `## Acceptance criteria`,
    `1. Engine remains paper_only; live_trading_authority=false.`,
    `2. Tests cover each disabled/demoted setup or tier cap.`,
    `3. Day expectancy after change is measured on journal replay / what-if.`,
    `4. No floating-point money types introduced.`,
    ``,
    `## Exclusions`,
    `- No auto-apply from MCP.`,
    `- No order signing or wallet access.`,
  );
  return lines.join("\n");
}

export function requireCitations(actions: SuggestionAction[]): {
  ok: boolean;
  missing: string[];
} {
  const missing: string[] = [];
  for (const a of actions) {
    const e = a.evidence ?? "";
    const hasNumber = /\d/.test(e);
    if (!hasNumber) missing.push(`${a.action}:${a.setup_id ?? "?"}`);
  }
  return { ok: missing.length === 0, missing };
}
