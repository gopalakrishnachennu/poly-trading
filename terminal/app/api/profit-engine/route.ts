import { createHash } from "node:crypto";
import { readFile, realpath } from "node:fs/promises";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

// Projects the latest immutable fixed-point BTC-hourly research cycle into the
// existing read-only terminal contract. It verifies automatic paper-only
// admission evidence, but never grants capital, signing or live authority.
export const dynamic = "force-dynamic";

type Latest = {
  schema_version: number;
  non_authoritative: boolean;
  revision: string;
  cycle_path: string;
  decision: "NO_TRADE" | "PAPER_AUTO_ADMITTED";
  generated_at_ms: number;
  cycle_report_sha256: string;
};

type FrozenSource = {
  source_size_at_freeze: number;
};

type CycleReport = {
  schema_version: number;
  non_authoritative: boolean;
  asset: "BTC";
  window: "1h";
  revision: string;
  generated_at_ms: number;
  decision: "NO_TRADE" | "PAPER_AUTO_ADMITTED";
  automatic_activation: boolean;
  paper_admission_status: "NO_ELIGIBLE_MODEL" | "AUTO_ADMITTED_PAPER_ONLY";
  paper_admission_sha256: string;
  local_completed_sessions: number;
  dataset_sessions: number;
  classifier_verdict: string;
  net_value_verdict: string;
  frozen_book_sources: FrozenSource[];
  frozen_reference_sources: FrozenSource[];
};

type PaperAdmission = {
  schema_version: number;
  paper_only: true;
  automatic: true;
  cycle_revision: string;
  status: "NO_ELIGIBLE_MODEL" | "AUTO_ADMITTED_PAPER_ONLY";
  paper_simulation_eligible: boolean;
  capital_authority: false;
  placement_authority: false;
  signing_authority: false;
  submission_authority: false;
  live_trading_authority: false;
};

function isHexDigest(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function boundedInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}

function validAdmission(value: unknown, report: CycleReport): value is PaperAdmission {
  if (!value || typeof value !== "object") return false;
  const admission = value as Partial<PaperAdmission>;
  const expectedStatus = report.decision === "PAPER_AUTO_ADMITTED"
    ? "AUTO_ADMITTED_PAPER_ONLY"
    : "NO_ELIGIBLE_MODEL";
  return (
    admission.schema_version === 1
    && admission.paper_only === true
    && admission.automatic === true
    && admission.cycle_revision === report.revision
    && admission.status === expectedStatus
    && admission.status === report.paper_admission_status
    && admission.paper_simulation_eligible
      === (expectedStatus === "AUTO_ADMITTED_PAPER_ONLY")
    && admission.capital_authority === false
    && admission.placement_authority === false
    && admission.signing_authority === false
    && admission.submission_authority === false
    && admission.live_trading_authority === false
  );
}

export async function GET() {
  const configuredRoot = process.env.POLY_RESEARCH_CYCLE_ROOT
    ?? join(process.cwd(), "..", "var", "research-cycles");
  const cycleRoot = resolve(configuredRoot);
  const latestPath = join(cycleRoot, "latest.json");
  try {
    const latest = JSON.parse(await readFile(latestPath, "utf8")) as Latest;
    if (
      latest.schema_version !== 1
      || latest.non_authoritative !== true
      || !isHexDigest(latest.revision)
      || !isHexDigest(latest.cycle_report_sha256)
      || !boundedInteger(latest.generated_at_ms)
      || !["NO_TRADE", "PAPER_AUTO_ADMITTED"].includes(latest.decision)
      || typeof latest.cycle_path !== "string"
    ) {
      throw new Error("latest research pointer contract invalid");
    }
    const requestedCycle = isAbsolute(latest.cycle_path)
      ? resolve(latest.cycle_path)
      : resolve(cycleRoot, latest.cycle_path);
    const canonicalRoot = await realpath(cycleRoot);
    const canonicalCycle = await realpath(requestedCycle);
    const traversal = relative(canonicalRoot, canonicalCycle);
    if (
      traversal === ".."
      || traversal.startsWith(`..${sep}`)
      || isAbsolute(traversal)
      || canonicalCycle !== join(canonicalRoot, latest.revision)
    ) {
      throw new Error("research cycle escaped configured root");
    }
    const reportRaw = await readFile(join(canonicalCycle, "cycle-report.json"));
    const digest = createHash("sha256").update(reportRaw).digest("hex");
    if (digest !== latest.cycle_report_sha256) {
      throw new Error("research cycle report checksum mismatch");
    }
    const report = JSON.parse(reportRaw.toString("utf8")) as CycleReport;
    if (
      report.schema_version !== 1
      || report.non_authoritative !== true
      || report.asset !== "BTC"
      || report.window !== "1h"
      || report.revision !== latest.revision
      || report.decision !== latest.decision
      || report.automatic_activation !== false
      || !["NO_ELIGIBLE_MODEL", "AUTO_ADMITTED_PAPER_ONLY"].includes(
        report.paper_admission_status,
      )
      || !isHexDigest(report.paper_admission_sha256)
      || !boundedInteger(report.generated_at_ms)
      || !boundedInteger(report.local_completed_sessions)
      || !boundedInteger(report.dataset_sessions)
      || typeof report.classifier_verdict !== "string"
      || typeof report.net_value_verdict !== "string"
      || !Array.isArray(report.frozen_book_sources)
      || !Array.isArray(report.frozen_reference_sources)
    ) {
      throw new Error("research cycle report contract invalid");
    }
    const admissionRaw = await readFile(join(canonicalCycle, "paper-admission.json"));
    const admissionDigest = createHash("sha256").update(admissionRaw).digest("hex");
    if (admissionDigest !== report.paper_admission_sha256) {
      throw new Error("paper admission checksum mismatch");
    }
    const admission = JSON.parse(admissionRaw.toString("utf8")) as unknown;
    if (!validAdmission(admission, report)) {
      throw new Error("paper admission contract invalid");
    }
    const sources = [
      ...report.frozen_book_sources,
      ...report.frozen_reference_sources,
    ];
    if (sources.some((source) => !boundedInteger(source.source_size_at_freeze))) {
      throw new Error("research source accounting invalid");
    }
    return Response.json({
      available: true,
      report: {
        generated_at_ms: report.generated_at_ms,
        duration_ms: 0,
        window: "1h",
        sources: {
          polymarket_files: report.frozen_book_sources.length,
          external_files: report.frozen_reference_sources.length,
          total_bytes: sources.reduce(
            (total, source) => total + source.source_size_at_freeze,
            0,
          ),
        },
        labels: {
          rows_in: 0,
          rows_with_resolution: 0,
          resolved_contracts: report.dataset_sessions,
          share_up_profitable: 0,
          share_down_profitable: 0,
        },
        walk_forward: {
          n_folds: 0,
          error: `Classifier: ${report.classifier_verdict}; net value: ${report.net_value_verdict}`,
        },
        verdict: report.decision,
        revision: report.revision,
        local_completed_sessions: report.local_completed_sessions,
        dataset_sessions: report.dataset_sessions,
        classifier_verdict: report.classifier_verdict,
        net_value_verdict: report.net_value_verdict,
        paper_admission_status: report.paper_admission_status,
        automatic_activation: false,
      },
    });
  } catch (error) {
    return Response.json({
      available: false,
      reason: error instanceof Error
        ? `fixed-point research cycle unavailable: ${error.message}`
        : "fixed-point research cycle unavailable",
    }, { status: 409 });
  }
}
