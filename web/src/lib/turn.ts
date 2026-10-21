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
  latencyMs: number | null;
  inputTokens: number | null;
  outputTokens: number | null;
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
  input_tokens: number | null;
  output_tokens: number | null;
  latency_ms: number | null;
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

type ReasonLabel = { decision: "permit" | "deny"; text: string };

// The gateway's policy reason codes, in a preparer's words, each with the decision it explains.
// The raw code is always shown beside it. These are every fixed code the gateway writes into a
// ledger row: crates/pru-gateway/src/lib.rs (execute_gateway_action, append_completion) and the
// Cedar authorizer's diagnostics in crates/pru-policy/src/lib.rs. tests/reasons.test.tsx reads
// both files and fails if this list and the Rust source disagree.
const REASONS = new Map<string, ReasonLabel>([
  ["cedar_allow", { decision: "permit", text: "Allowed by the consent on file." }],
  ["upstream_complete", { decision: "permit", text: "The allowed model call finished." }],
  ["no_active_consent", { decision: "deny", text: "No consent is on file for this client." }],
  ["matching_consent_required", { decision: "deny", text: "No matching use consent is on file for this client." }],
  ["consent_facts_do_not_match", { decision: "deny", text: "No consent on file names this recipient and purpose." }],
  ["ssn_requires_local_destination", { decision: "deny", text: "An SSN may only go to the local model." }],
  ["token_factory_not_configured", { decision: "deny", text: "Token Factory is not configured on this server." }],
  ["token_client_mismatch", { decision: "deny", text: "The consent token belongs to a different client." }],
  ["token_purpose_mismatch", { decision: "deny", text: "The consent token names a different purpose." }],
  ["principal_not_pru_agent", { decision: "deny", text: "Only Pru's own agent may move client data." }],
]);

// A failed consent verification is written as the error's own sentence with a value appended
// (ConsentError in crates/pru-consent/src/lib.rs), so these match on the fixed start.
const REASON_PREFIXES: ReadonlyArray<readonly [string, ReasonLabel]> = [
  ["consent expired at ", { decision: "deny", text: "The client's consent has expired." }],
  ["consent token is revoked: ", { decision: "deny", text: "The client revoked this consent." }],
  ["invalid consent record: ", { decision: "deny", text: "The consent record on the token is not valid." }],
];

// What a reason with no label shows, and a label written for the other decision. It states only
// that the gateway recorded the reason, so it cannot contradict the decision stamp beside it.
export const UNLABELED_REASON = "The gateway recorded this reason:";

function reasonLabel(code: string, decision: string): string | null {
  const label = REASONS.get(code) ?? REASON_PREFIXES.find(([prefix]) => code.startsWith(prefix))?.[1];
  return label && label.decision === decision ? label.text : null;
}

export function reasonText(code: string, decision: string): string {
  return reasonLabel(code, decision) ?? UNLABELED_REASON;
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
  if (summary.denied) {
    const { reason } = summary.denied;
    return reasonLabel(reason, "deny") ?? `${UNLABELED_REASON} ${reason}`;
  }
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

// A measured value the source did not record reads as unknown, never as zero.
export const UNKNOWN_VALUE = "unknown";

export function latencyText(step: TraceStep): string {
  return step.latencyMs === null ? UNKNOWN_VALUE : `${step.latencyMs} ms`;
}

export function tokensText(step: TraceStep): string {
  if (step.inputTokens === null && step.outputTokens === null) return UNKNOWN_VALUE;
  return `${step.inputTokens ?? UNKNOWN_VALUE} in / ${step.outputTokens ?? UNKNOWN_VALUE} out`;
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
