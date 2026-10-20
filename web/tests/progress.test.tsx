import { act, cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ENDPOINT_TEST } from "../src/lib/measured";
import { PRESETS } from "../src/ui/JudgeDoor";
import { localTurnNote, RouteTradeoff, RunProgress } from "../src/ui/Progress";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("turn progress", () => {
  it("counts elapsed seconds while a turn runs", () => {
    vi.useFakeTimers();
    render(<RunProgress prompt="Read the client's W-2" isLocal={false} hosted={false} startedAt={Date.now()} message="" />);
    expect(screen.getByRole("timer")).toHaveTextContent("0 s elapsed");
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("2 s elapsed");
    act(() => {
      vi.advanceTimersByTime(9_000);
    });
    expect(screen.getByRole("timer")).toHaveTextContent("11 s elapsed");
  });

  it("adds the measured local note only for a request that names the SSN", () => {
    render(<RunProgress prompt="Check this client's SSN" isLocal hosted={false} startedAt={0} message="Pru is checking consent and selecting a model." />);
    expect(document.querySelector("[data-local-note]")?.textContent).toBe(localTurnNote(false));
    expect(document.querySelector("[data-run-message]")?.textContent).toBe("Pru is checking consent and selecting a model.");
    cleanup();
    render(<RunProgress prompt="Read the client's W-2" isLocal={false} hosted={false} startedAt={0} message="" />);
    expect(document.querySelector("[data-local-note]")).toBeNull();
  });
});

describe("measured route trade-offs", () => {
  it("shows exactly the endpoint test's numbers and says how cost was computed", () => {
    render(<RouteTradeoff hosted={false} />);
    const table = screen.getByRole("table");
    const local = within(table).getByRole("row", { name: /^Local/ });
    expect(local).toHaveTextContent(`${ENDPOINT_TEST.local.model}`);
    expect(local).toHaveTextContent("11.1 s first, 5.0 s repeat");
    expect(local).toHaveTextContent("No API fee");
    const hosted = within(table).getByRole("row", { name: /^Token Factory/ });
    expect(hosted).toHaveTextContent(ENDPOINT_TEST.tokenFactory.model);
    expect(hosted).toHaveTextContent("2.9 s, 3 calls of 315 to 640 ms");
    expect(hosted).toHaveTextContent("$0.00046");
    const caption = screen.getByText(/^Measured 2026-10-20 on a Nebius cpu-d3 endpoint, 4 vCPU\./);
    expect(caption).toHaveTextContent("Cost is computed from token counts at list rates, not billed.");
    expect(caption).toHaveTextContent("Times on this PC will differ.");
  });

  it("drops the PC caveat on the hosted door", () => {
    render(<RouteTradeoff hosted />);
    expect(screen.queryByText(/Times on this PC will differ/)).toBeNull();
  });

  it("uses the same measured figures in the local note and the local preset", () => {
    expect(localTurnNote(false)).toBe(
      "This request names the SSN, so the local Nemotron 3 Nano 4B reads it on this PC's CPU. " +
      "Measured on a Nebius cpu-d3 endpoint, 4 vCPU: 11.1 s for a first local turn, 5.0 s for a repeat.",
    );
    expect(localTurnNote(true)).toContain("on this server's CPU");
    const localPreset = PRESETS.find((preset) => preset.prompt.includes("SSN"));
    expect(localPreset?.outcome).toContain("measured at 5.0 s to 11.1 s.");
  });
});
