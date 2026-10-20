import type { ChatStep } from "@/lib/contracts";

// One line of a turn's trace, whether it came from a live gateway response or a committed receipt.
// Fields a source does not record stay null rather than being inferred.
export type TraceStep = {
  key: string;
  kind: string;
  toolName: string | null;
  decision: string;
  route: string;
  reason: string;
  redactedSpans: number;
  latencyMs: number;
  inputTokens: number;
  outputTokens: number;
  modelId: string | null;
  costUsd: number | null;
  dryRun: boolean;
};

// A gateway decision as written in eval/hermes_turns-*.json.
export type RecordedDecision = {
  ledger_entry_id: string;
  actor: string;
  step: string;
  tool: string | null;
  decision: string;
  route: string;
  policy_reason: string;
  contains_ssn: boolean;
  redacted_spans: number;
  input_tokens: number;
  output_tokens: number;
  latency_ms: number;
};

export function fromChatStep(step: ChatStep): TraceStep {
  return {
    key: step.ledger_entry_id,
    kind: step.kind,
    toolName: step.tool_name,
    decision: step.decision,
    route: step.route,
    reason: step.policy_reason,
    redactedSpans: step.redacted_spans ?? 0,
    latencyMs: step.latency_ms,
    inputTokens: step.input_tokens,
    outputTokens: step.output_tokens,
    modelId: step.model_id,
    costUsd: step.cost_usd,
    dryRun: step.dry_run,
  };
}

export function fromRecordedDecision(decision: RecordedDecision): TraceStep {
  return {
    key: decision.ledger_entry_id,
    kind: decision.step,
    toolName: decision.tool,
    decision: decision.decision,
    route: decision.route,
    reason: decision.policy_reason,
    redactedSpans: decision.redacted_spans,
    latencyMs: decision.latency_ms,
    inputTokens: decision.input_tokens,
    outputTokens: decision.output_tokens,
    modelId: null,
    costUsd: null,
    dryRun: decision.route === "dry_run",
  };
}

// The gateway's policy reason codes, in a preparer's words. The raw code is always shown beside it.
const REASONS: Record<string, string> = {
  cedar_allow: "Allowed by the consent on file.",
  no_active_consent: "No consent is on file for this client.",
  matching_consent_required: "No matching use consent is on file for this client.",
  consent_facts_do_not_match: "No consent on file names this recipient and purpose.",
  ssn_requires_local_destination: "An SSN may only go to the local model.",
  rate_limit_exceeded: "Too many requests this minute.",
  token_factory_not_configured: "Token Factory is not configured on this server.",
};

export function reasonText(code: string): string {
  return REASONS[code] ?? "Pru refused this step.";
}

export type RouteStamp = "LOCAL" | "TOKEN FACTORY";

export type TurnSummary = {
  decision: "ALLOWED" | "DENIED";
  routes: RouteStamp[];
  denied: TraceStep | null;
  redactedSpans: number;
  dryRunTools: string[];
};

export function summarizeTurn(steps: readonly TraceStep[]): TurnSummary {
  const denied = steps.find((step) => step.decision === "deny") ?? null;
  // A route counts only where the gateway let data go. A refused call keeps its intended route in
  // the ledger, but nothing reached that model.
  const permitted = steps.filter((step) => step.decision === "permit");
  const routes: RouteStamp[] = [];
  if (permitted.some((step) => step.route === "local")) routes.push("LOCAL");
  if (permitted.some((step) => step.route === "token_factory")) routes.push("TOKEN FACTORY");
  return {
    decision: denied ? "DENIED" : "ALLOWED",
    routes,
    denied,
    redactedSpans: steps.reduce((total, step) => total + step.redactedSpans, 0),
    dryRunTools: steps
      .filter((step) => step.dryRun && step.decision === "permit" && step.toolName)
      .map((step) => step.toolName as string),
  };
}

const DRY_RUN_RESULTS: Record<string, string> = {
  draft_client_email: "Drafted, not sent.",
  propose_calendar_slot: "Proposed, not booked.",
};

// The plain sentence under a turn's stamps, built only from what its steps record.
export function verdictSentence(summary: TurnSummary): string {
  if (summary.denied) return reasonText(summary.denied.reason);
  const parts: string[] = [];
  if (summary.routes.includes("LOCAL")) parts.push("The SSN-bearing step ran on the local model.");
  for (const tool of new Set(summary.dryRunTools)) {
    parts.push(DRY_RUN_RESULTS[tool] ?? `${tool} ran as a dry run, nothing sent.`);
  }
  if (summary.redactedSpans > 0) {
    parts.push(`${summary.redactedSpans} SSN ${summary.redactedSpans === 1 ? "span was" : "spans were"} removed before the data went to Token Factory.`);
  }
  return parts.length > 0 ? parts.join(" ") : "Answered under the consent on file.";
}

export function stepTitle(step: TraceStep): string {
  if (step.toolName) return `Tool: ${step.toolName}`;
  if (step.kind === "model" || step.route === "token_factory" || step.route === "local") return "Model call";
  return step.kind.replaceAll("_", " ");
}

export function decisionLabel(decision: string): string {
  if (decision === "permit") return "ALLOWED";
  if (decision === "deny") return "DENIED";
  return decision.toUpperCase();
}
