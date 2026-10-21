import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ChatStep } from "../src/lib/contracts";
import { fromChatStep } from "../src/lib/turn";
import { TraceList } from "../src/ui/Turn";

// The ask_local_model step as the gateway returns it, carrying the time and token counts the
// gallery reshoot's llama-server log recorded for that call.
const localStep: ChatStep = {
  kind: "tool",
  route: "local",
  model_id: "nemotron-3-nano-4b",
  latency_ms: 8439,
  input_tokens: 179,
  output_tokens: 26,
  cost_usd: 0,
  decision: "permit",
  policy_reason: "cedar_allow",
  ledger_entry_id: "c31490ac-979e-44ed-915b-d7d3d540444a",
  dry_run: false,
  tool_name: "ask_local_model",
  redacted_spans: 0,
};

function shownValue(row: HTMLElement, term: string): string {
  const label = within(row).getByText(term, { selector: "dt" });
  return label.nextElementSibling?.textContent ?? "";
}

function renderStep(step: ChatStep): HTMLElement {
  render(<TraceList steps={[fromChatStep(step)]} label="Turn trace" />);
  return screen.getByRole("listitem");
}

afterEach(cleanup);

describe("a trace step's measured values", () => {
  it("shows the local model call's time, tokens and zero cost", () => {
    const row = renderStep(localStep);
    expect(shownValue(row, "Latency")).toBe("8439 ms");
    expect(shownValue(row, "Tokens")).toBe("179 in / 26 out");
    expect(shownValue(row, "Cost")).toBe("$0.00000000");
  });

  it("shows a value the gateway did not record as unknown, never as 0", () => {
    const row = renderStep({ ...localStep, latency_ms: null, input_tokens: null, output_tokens: null });
    expect(shownValue(row, "Latency")).toBe("unknown");
    expect(shownValue(row, "Tokens")).toBe("unknown");
    expect(row.textContent).not.toMatch(/\b0 ms|\b0 in\b|\b0 out\b/);
  });

  it("names which token count is unknown when only one is", () => {
    const row = renderStep({ ...localStep, input_tokens: null });
    expect(shownValue(row, "Tokens")).toBe("unknown in / 26 out");
  });
});
