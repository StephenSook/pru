import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";

import { ENDPOINT_TEST } from "@/lib/measured";

const RECEIPT = path.join(process.cwd(), "..", "eval", "endpoint_test-2026-10-20");
type Step = { kind: string; route: string; latency_ms: number; cost_usd: number };

function steps(file: string): Step[] {
  const events = readFileSync(path.join(RECEIPT, file), "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
  return JSON.parse(events.at(-1).body).steps as Step[];
}

const summary = JSON.parse(readFileSync(path.join(RECEIPT, "summary.json"), "utf8")) as {
  turns: Record<string, { wall_seconds: number }>;
};

describe("the route trade-off figures on the judge door", () => {
  test("match the saved endpoint-test wall times", () => {
    expect(summary.turns.turnL1.wall_seconds.toFixed(1)).toBe(ENDPOINT_TEST.local.firstTurnSeconds.toFixed(1));
    expect(summary.turns.turnL2.wall_seconds.toFixed(1)).toBe(ENDPOINT_TEST.local.repeatTurnSeconds.toFixed(1));
    expect(summary.turns.turn1.wall_seconds.toFixed(1)).toBe(ENDPOINT_TEST.tokenFactory.turnSeconds.toFixed(1));
  });

  test("match the Token Factory calls of that turn", () => {
    const calls = steps("turn1.ndjson").filter((step) => step.kind === "model" && step.route === "token_factory");
    expect(calls).toHaveLength(ENDPOINT_TEST.tokenFactory.calls);
    expect(Math.min(...calls.map((call) => call.latency_ms))).toBe(ENDPOINT_TEST.tokenFactory.callMsMin);
    expect(Math.max(...calls.map((call) => call.latency_ms))).toBe(ENDPOINT_TEST.tokenFactory.callMsMax);
    expect(Number(calls.reduce((sum, call) => sum + call.cost_usd, 0).toFixed(5))).toBe(ENDPOINT_TEST.tokenFactory.turnCostUsd);
  });
});
