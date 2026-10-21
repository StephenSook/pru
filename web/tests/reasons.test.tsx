import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { LedgerEntry } from "../src/lib/contracts";
import { UNLABELED_REASON, reasonText, summarizeTurn, verdictSentence, type TraceStep } from "../src/lib/turn";
import { LedgerRow } from "../src/ui/JudgeDoor";
import { TraceList } from "../src/ui/Turn";

// The gateway's Rust source lives outside web/, so these tests read it from the checkout, as
// tests/recorded-turn.test.tsx reads the eval receipt. Vitest runs from web/ locally and in CI.
const repoRoot = resolve(process.cwd(), "..");
const source = (path: string) => readFileSync(resolve(repoRoot, path), "utf8");

// Every fixed reason code the gateway writes into a ledger row, and the decision it is written
// with. The first test below checks this list against the Rust source, so it cannot drift.
const GATEWAY_REASONS: ReadonlyArray<readonly [string, "permit" | "deny"]> = [
  ["cedar_allow", "permit"],
  ["upstream_complete", "permit"],
  ["no_active_consent", "deny"],
  ["matching_consent_required", "deny"],
  ["consent_facts_do_not_match", "deny"],
  ["ssn_requires_local_destination", "deny"],
  ["token_factory_not_configured", "deny"],
  ["token_client_mismatch", "deny"],
  ["token_purpose_mismatch", "deny"],
  ["principal_not_pru_agent", "deny"],
];

// Reads the codes from where the gateway writes them: policy_reason literals and *_REASON
// constants in crates/pru-gateway/src/lib.rs, and the Cedar authorizer's diagnostics in
// crates/pru-policy/src/lib.rs.
function reasonCodesInRustSource(): string[] {
  const gateway = source("crates/pru-gateway/src/lib.rs");
  const policy = source("crates/pru-policy/src/lib.rs");
  const codes = new Set<string>();
  for (const [, code] of gateway.matchAll(/policy_reason:\s*"([a-z_]+)"/g)) codes.add(code);
  for (const [, code] of gateway.matchAll(/pub const \w+_REASON: &str = "([a-z_]+)"/g)) codes.add(code);
  for (const [, code] of policy.matchAll(/diagnostics:\s*vec!\["([a-z_]+)"/g)) codes.add(code);
  const fallback = policy.match(/let diagnostic =([\s\S]*?)\};/)?.[1] ?? "";
  for (const [, code] of fallback.matchAll(/^\s*"([a-z_]+)"\s*$/gm)) codes.add(code);
  return [...codes].sort();
}

function step(overrides: Partial<TraceStep>): TraceStep {
  return {
    key: crypto.randomUUID(),
    kind: "model",
    toolName: null,
    decision: "permit",
    route: "token_factory",
    reason: "cedar_allow",
    redactedSpans: 0,
    latencyMs: 12,
    inputTokens: 100,
    outputTokens: 20,
    modelId: null,
    costUsd: null,
    dryRun: false,
    ...overrides,
  };
}

const REFUSAL = /refus|denied|not configured|may only|no consent|no matching|revoked|expired|not valid|different|only pru/i;

afterEach(cleanup);

describe("gateway reason labels", () => {
  it("covers exactly the reason codes the Rust gateway and policy write", () => {
    const codes = reasonCodesInRustSource();
    expect(codes.length).toBeGreaterThanOrEqual(GATEWAY_REASONS.length);
    expect(codes).toEqual(GATEWAY_REASONS.map(([code]) => code).sort());
  });

  it("gives every gateway reason a label for the decision it is written with", () => {
    for (const [code, decision] of GATEWAY_REASONS) {
      expect(reasonText(code, decision), code).not.toBe(UNLABELED_REASON);
    }
  });

  it("never puts refusal words next to an ALLOWED row", () => {
    expect(reasonText("upstream_complete", "permit")).not.toMatch(REFUSAL);
    for (const code of [...GATEWAY_REASONS.map(([code]) => code), "a_code_added_later", "", "toString"]) {
      expect(reasonText(code, "permit"), code).not.toMatch(REFUSAL);
    }
  });

  it("does not show a label written for the other decision", () => {
    for (const [code, decision] of GATEWAY_REASONS) {
      const other = decision === "permit" ? "deny" : "permit";
      expect(reasonText(code, other), code).toBe(UNLABELED_REASON);
    }
  });

  it("shows an unknown reason as neutral, never as a refusal", () => {
    expect(UNLABELED_REASON).not.toMatch(REFUSAL);
    expect(UNLABELED_REASON).not.toMatch(/allow/i);
    for (const decision of ["permit", "deny", "pending"]) {
      expect(reasonText("a_code_added_later", decision)).toBe(UNLABELED_REASON);
    }
  });

  it("labels the consent verification errors the gateway writes as their own sentences", () => {
    const consent = source("crates/pru-consent/src/lib.rs");
    expect(consent).toContain('#[error("consent expired at {0}")]');
    expect(consent).toContain('#[error("consent token is revoked: {0}")]');
    expect(consent).toContain('#[error("invalid consent record: {0}")]');
    expect(reasonText("consent expired at 2026-10-21 00:00:00 UTC", "deny")).toBe("The client's consent has expired.");
    expect(reasonText("consent token is revoked: 4e70c564", "deny")).toBe("The client revoked this consent.");
    expect(reasonText("invalid consent record: purpose is empty", "deny")).toBe("The consent record on the token is not valid.");
  });

  it("keeps a denied turn's sentence free of a guessed label", () => {
    const summary = summarizeTurn([step({ decision: "deny", reason: "a_code_added_later" })]);
    expect(verdictSentence(summary)).toBe(`${UNLABELED_REASON} a_code_added_later`);
  });
});

describe("rows that show a reason", () => {
  it("shows a finished Token Factory call in the ledger as allowed, not refused", () => {
    const entry: LedgerEntry = {
      id: "6a1b7c11-0d3e-4c0e-9a55-0d3d9b1c2e01",
      at: "2026-10-20T21:00:00Z",
      client_id: "demo-avery",
      action: "agent_model",
      decision: "permit",
      route: "token_factory",
      policy_reason: "upstream_complete",
      model_id: "nvidia/Nemotron-3_5-Lightning",
      contains_ssn: false,
      span_hashes: [],
    };
    render(<LedgerRow entry={entry} />);
    const row = screen.getByRole("article");
    expect(row).toHaveTextContent("ALLOWED");
    expect(row).toHaveTextContent("(upstream_complete)");
    expect(row.textContent).not.toMatch(REFUSAL);
  });

  it("shows an allowed trace step with a completion reason without refusal words", () => {
    render(<TraceList steps={[step({ reason: "upstream_complete" })]} label="Turn trace" />);
    const row = screen.getByRole("listitem");
    expect(row).toHaveTextContent("ALLOWED");
    expect(row.textContent).not.toMatch(REFUSAL);
  });
});
