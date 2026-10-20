import { describe, expect, it } from "vitest";
import { recordedSteps } from "../src/lib/recorded";
import { summarizeTurn, verdictSentence, type TraceStep } from "../src/lib/turn";

function step(overrides: Partial<TraceStep>): TraceStep {
  return {
    key: crypto.randomUUID(),
    kind: "model",
    toolName: null,
    decision: "permit",
    route: "token_factory",
    reason: "cedar_allow",
    redactedSpans: 0,
    latencyMs: 0,
    inputTokens: 0,
    outputTokens: 0,
    modelId: null,
    costUsd: null,
    dryRun: false,
    ...overrides,
  };
}

describe("turn verdicts", () => {
  it("stamps a refused first call DENIED and claims no route, since nothing reached a model", () => {
    const summary = summarizeTurn([step({ decision: "deny", reason: "no_active_consent" })]);
    expect(summary.decision).toBe("DENIED");
    expect(summary.routes).toEqual([]);
    expect(verdictSentence(summary)).toBe("No consent is on file for this client.");
  });

  it("is DENIED when any step is refused, and keeps the routes that permitted steps used", () => {
    const summary = summarizeTurn([
      step({}),
      step({ kind: "tool", toolName: "draft_client_email", route: "dry_run", dryRun: true, decision: "deny", reason: "consent_facts_do_not_match" }),
    ]);
    expect(summary.decision).toBe("DENIED");
    expect(summary.routes).toEqual(["TOKEN FACTORY"]);
    expect(verdictSentence(summary)).toBe("No consent on file names this recipient and purpose.");
  });

  it("names the local route first and says the SSN-bearing step stayed local", () => {
    const summary = summarizeTurn([step({}), step({ kind: "tool", toolName: "ask_local_model", route: "local" })]);
    expect(summary.decision).toBe("ALLOWED");
    expect(summary.routes).toEqual(["LOCAL", "TOKEN FACTORY"]);
    expect(verdictSentence(summary)).toBe("The SSN-bearing step ran on the local model.");
  });

  it("summarizes the recorded W-2 turn from its own decisions", () => {
    const summary = summarizeTurn(recordedSteps());
    expect(summary.decision).toBe("ALLOWED");
    expect(summary.routes).toEqual(["TOKEN FACTORY"]);
    expect(summary.dryRunTools).toEqual(["draft_client_email"]);
    expect(verdictSentence(summary)).toBe(
      "Drafted, not sent. 1 SSN span was removed before the data went to Token Factory.",
    );
  });
});
