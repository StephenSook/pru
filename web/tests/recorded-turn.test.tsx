import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import extract from "../src/data/recorded-turn.json";
import { probeSummary, recordedDate } from "../src/lib/recorded";
import { RecordedTurn } from "../src/ui/Turn";

// The receipt and the eval script live outside web/, so these tests read them from the checkout.
// Vitest runs from web/ locally and in CI.
const repoRoot = resolve(process.cwd(), "..");
const receiptPath = resolve(repoRoot, "eval", "hermes_turns-2026-10-20.json");
const scriptPath = resolve(repoRoot, "scripts", "run-hermes-eval.py");
const receipt = JSON.parse(readFileSync(receiptPath, "utf8"));
const lightning = receipt.candidates.lightning;

afterEach(cleanup);

describe("the recorded turn copied into web/src/data", () => {
  it("names the receipt it was copied from", () => {
    expect(extract.source).toBe("eval/hermes_turns-2026-10-20.json");
    expect(extract.generated_at).toBe(receipt.generated_at);
    expect(recordedDate()).toBe("2026-10-20");
  });

  it("is the receipt's own w2_to_marketing turn, field for field", () => {
    const original = lightning.turns.find(
      (turn: { prompt_id: string; run: number }) => turn.prompt_id === extract.turn.prompt_id && turn.run === extract.turn.run,
    );
    expect(extract.turn.prompt_id).toBe("w2_to_marketing");
    expect(extract.turn).toEqual(original);
    expect(extract.prompt).toEqual(lightning.prompts.find((prompt: { id: string }) => prompt.id === "w2_to_marketing"));
  });

  it("copies the run's agent, model, cost basis and security probes unchanged", () => {
    expect(extract.candidate).toBe(lightning.environment.candidate);
    expect(extract.agent).toBe(lightning.environment.health.agent);
    expect(extract.model).toBe(lightning.environment.candidate_model);
    expect(extract.cost_basis).toBe(lightning.summary.cost_basis);
    expect(extract.security_probes).toEqual(lightning.security_probes);
  });

  it("states the consents the eval script recorded before this prompt", () => {
    const script = readFileSync(scriptPath, "utf8");
    const block = script.match(/"id": "w2_to_marketing",[\s\S]*?"disclose_recipient": "([^"]+)"/);
    expect(block?.[1]).toBe(extract.eval_setup.disclose_recipient);
    expect(script).toMatch(/"kind": "use",/);
    expect(extract.eval_setup.source).toBe("scripts/run-hermes-eval.py");
  });
});

describe("the recorded turn as shown", () => {
  it("shows the receipt's answer, decisions and timings and nothing invented", () => {
    render(<RecordedTurn />);
    const card = screen.getByRole("article", { name: "Recorded turn" });
    expect(within(card).getByText(`Recorded 2026-10-20 from ${extract.source}`)).toBeInTheDocument();
    expect(card.querySelector("[data-recorded-answer]")?.textContent).toBe(extract.turn.answer);

    const trace = within(card).getByRole("list", { name: "Recorded turn trace" });
    const rows = within(trace).getAllByRole("listitem");
    expect(rows).toHaveLength(extract.turn.gateway_decisions.length);
    extract.turn.gateway_decisions.forEach((decision, index) => {
      const row = rows[index];
      expect(row).toHaveTextContent(`${decision.latency_ms} ms`);
      expect(row).toHaveTextContent(`${decision.input_tokens} in / ${decision.output_tokens} out`);
      expect(row).toHaveTextContent(decision.policy_reason);
      expect(row).toHaveTextContent(decision.decision === "permit" ? "ALLOWED" : "DENIED");
    });

    const meta = card.querySelector("[data-recorded-meta]")?.textContent ?? "";
    expect(meta).toContain(`${extract.turn.latency_ms.wall} ms end to end`);
    expect(meta).toContain(`$${extract.turn.estimated_cost_usd}`);
    expect(meta).toContain(extract.cost_basis);
    expect(meta).toContain(extract.model);
  });

  it("reports the probes and SSN egress from the receipt's own fields", () => {
    render(<RecordedTurn />);
    const probes = probeSummary();
    expect(probes.total).toBe(lightning.security_probes.length);
    expect(probes.denied).toBe(lightning.security_probes.filter((probe: { passed: boolean }) => probe.passed).length);
    expect(document.querySelector("[data-recorded-probes]")?.textContent).toContain(
      `${probes.denied} of ${probes.total} attempts to send an SSN to Token Factory were denied (ssn_requires_local_destination)`,
    );
    expect(extract.turn.ssn_reached_token_factory).toBe(false);
    expect(document.querySelector("[data-recorded-ssn]")?.textContent).toContain("No SSN reached Token Factory.");
  });
});
