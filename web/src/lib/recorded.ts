import extract from "@/data/recorded-turn.json";
import { fromRecordedDecision, type RecordedDecision, type TraceStep } from "@/lib/turn";

// A field-for-field copy of one turn from the committed receipt eval/hermes_turns-2026-10-20.json
// (candidate "lightning", prompt "w2_to_marketing", run 2), plus that run's SSN security probes.
// The Docker web build sees only web/, so the copy lives here; tests/recorded-turn.test.tsx
// fails if any copied field drifts from the receipt.
export type RecordedProbe = {
  id: string;
  http_status: number;
  token_factory_rows: number;
  ssn_bearing_rows: number;
  permitted_ssn_rows: number;
  deny_reasons: string[];
  hosted_input_tokens: number;
  hosted_output_tokens: number;
  estimated_cost_usd: number;
  passed: boolean;
};

export type RecordedExtract = {
  source: string;
  generated_at: string;
  candidate: string;
  agent: string;
  model: string;
  cost_basis: string;
  prompt: { id: string; text: string; expected_first_tool: string };
  turn: {
    prompt_id: string;
    run: number;
    turn_id: string;
    answer: string;
    tools_called: string[];
    observed_first_tool: string;
    expected_first_tool: string;
    grounded: boolean;
    gateway_decisions: RecordedDecision[];
    tokens: Record<string, number>;
    latency_ms: { wall: number; gateway_steps: number; model: number; tools: number };
    ssn_reached_token_factory: boolean;
    estimated_cost_usd: number;
  };
  security_probes: RecordedProbe[];
  // What the eval set up before this prompt, from scripts/run-hermes-eval.py: a use consent for
  // every prompt, and a disclose consent naming this prompt's recipient.
  eval_setup: { source: string; use_consent: boolean; disclose_recipient: string };
};

export const RECORDED: RecordedExtract = extract;

// The receipt's own UTC date, so the label cannot claim a day the file does not record.
export function recordedDate(record: RecordedExtract = RECORDED): string {
  return record.generated_at.slice(0, 10);
}

export function recordedSteps(record: RecordedExtract = RECORDED): TraceStep[] {
  return record.turn.gateway_decisions.map(fromRecordedDecision);
}

export function probeSummary(record: RecordedExtract = RECORDED): { denied: number; total: number; reasons: string[] } {
  const probes = record.security_probes;
  return {
    denied: probes.filter((probe) => probe.passed && probe.permitted_ssn_rows === 0).length,
    total: probes.length,
    reasons: [...new Set(probes.flatMap((probe) => probe.deny_reasons))],
  };
}
