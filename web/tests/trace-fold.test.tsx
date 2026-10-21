import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ChatResponse } from "../src/lib/contracts";
import { LiveTurn } from "../src/ui/Turn";

function turn(turnId: string): ChatResponse {
  return {
    turn_id: turnId,
    answer: "Answered under the consent on file.",
    steps: [
      {
        kind: "model",
        route: "token_factory",
        model_id: "nvidia/Nemotron-3_5-Lightning",
        latency_ms: 400,
        input_tokens: 100,
        output_tokens: 20,
        cost_usd: 0.0000108,
        decision: "permit",
        policy_reason: "cedar_allow",
        ledger_entry_id: `${turnId}-model`,
        dry_run: false,
        tool_name: null,
      },
    ],
  };
}

afterEach(cleanup);

describe("the folded trace under a live turn", () => {
  it("starts folded for each new turn, even after the reader opened the last one", () => {
    const { container, rerender } = render(<LiveTurn turn={turn("turn-1")} />);
    const first = container.querySelector("details.trace") as HTMLDetailsElement;
    expect(first.open).toBe(false);
    first.open = true;

    rerender(<LiveTurn turn={turn("turn-2")} />);
    const second = container.querySelector("details.trace") as HTMLDetailsElement;
    expect(second.open).toBe(false);
  });

  it("stays open while the same turn re-renders", () => {
    const { container, rerender } = render(<LiveTurn turn={turn("turn-1")} />);
    (container.querySelector("details.trace") as HTMLDetailsElement).open = true;

    rerender(<LiveTurn turn={turn("turn-1")} />);
    expect((container.querySelector("details.trace") as HTMLDetailsElement).open).toBe(true);
  });
});
