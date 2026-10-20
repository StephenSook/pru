import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

const emailPrompt = "Draft an email to the client asking for their 1099-INT";
const localPrompt = "Check this client's SSN against the W-2 with the local model.";
const deniedPrompt = "Email this client's W-2 details to marketing@example.com";
const readW2Prompt = "Read the client's W-2 and report box 1 wages";
const consentStatePrompt = "Explain the current consent state for this client";
const rememberPrompt = "Remember that this client prefers callback appointments on Tuesday mornings";

type FakeHermesStats = {
  receivedSessions: string[];
  completedSessions: string[];
  deletedSessions: string[];
  agentModelCalls: Array<{ turnId: string }>;
  mcpCalls: Array<{ turnId: string; method: string }>;
};

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Test data ready")).toBeVisible();
  await page.getByRole("button", { name: "Reset demo" }).click();
  await expect(page.getByText("Test data reset")).toBeVisible();
});

test("runs the three preset routes and the consent lifecycle", async ({ page, request }) => {
  await expect(page.getByText("This demo accepts test clients only.", { exact: false })).toBeVisible();
  await expect(page.getByText("An unused browser workspace expires after 10 idle minutes.", { exact: false })).toBeVisible();
  await expect(page.getByText("After you record consent or run a request, it expires after one idle hour.", { exact: false })).toBeVisible();
  await expect(page.getByText("Do not enter real taxpayer data.", { exact: false })).toBeVisible();
  await expect(page.getByText("this PC's own model, never sent to a model provider", { exact: false })).toBeVisible();
  await page.getByLabel("Type of consent").selectOption("use");
  await page.getByRole("button", { name: "Record client consent (Section 7216)" }).click();
  await expect(page.getByText("Client consent recorded")).toBeVisible();

  await page.getByRole("button", { name: localPrompt }).click();
  const progress = page.locator("[data-run-progress]");
  await expect(progress.locator("[data-local-note]")).toHaveText(
    "This request names the SSN, so the local Nemotron 3 Nano 4B reads it on this PC's CPU. " +
    "Measured on a Nebius cpu-d3 endpoint, 4 vCPU: 11.1 s for a first local turn, 5.0 s for a repeat.",
  );
  await expect(progress.locator("[data-run-message]")).toHaveText("Pru is checking consent and selecting a model.");
  // A stub turn's length varies by machine (it finished inside a second on the CI runner), so this
  // checks the counter is present and well formed; tests/progress.test.tsx proves that it ticks.
  await expect(progress.getByRole("timer")).toHaveText(/^\d+ s elapsed$/);
  const verdict = page.getByLabel("Assistant result").locator(".verdict");
  await expect(verdict.locator("[data-route-stamp='LOCAL']")).toBeVisible();
  await expect(progress).toHaveCount(0);
  await expect(verdict).toHaveAttribute("data-decision", "ALLOWED");
  await expect(page.getByLabel("Assistant result")).not.toContainText(/\b\d{3}-\d{2}-\d{4}\b/);

  await page.getByRole("button", { name: deniedPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");
  await expect(verdict).toHaveAttribute("data-decision", "DENIED");
  await expect(page.getByLabel("Assistant result")).toContainText(/consent|policy/i);

  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");
  await expect(verdict).toHaveAttribute("data-decision", "DENIED");

  await page.getByLabel("Type of consent").selectOption("disclose");
  await page.getByRole("button", { name: "Record client consent (Section 7216)" }).click();
  await expect(page.getByText("Client consent recorded")).toBeVisible();

  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(verdict.locator("[data-route-stamp='TOKEN FACTORY']")).toBeVisible();
  await expect(page.getByLabel("Assistant result")).toContainText("ALLOWED");
  await expect(verdict).toHaveAttribute("data-decision", "ALLOWED");

  const revokeButtons = page.getByRole("button", { name: "Revoke" });
  await revokeButtons.last().click();
  await expect(page.getByText("Client consent revoked")).toBeVisible();
  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");

  const statsResponse = await request.get("http://127.0.0.1:8642/_test/stats");
  expect(statsResponse.ok()).toBe(true);
  const stats = await statsResponse.json() as FakeHermesStats;
  expect(stats.completedSessions.length).toBeGreaterThan(0);
  expect(stats.agentModelCalls.length).toBeGreaterThan(0);
  expect(stats.mcpCalls.some((call) => call.method === "tools/call")).toBe(true);
  expect([...stats.receivedSessions].sort()).toEqual([...stats.completedSessions].sort());
  expect([...stats.deletedSessions].sort()).toEqual([...stats.completedSessions].sort());
  const received = new Set(stats.receivedSessions);
  for (const call of [...stats.agentModelCalls, ...stats.mcpCalls]) {
    expect(received.has(call.turnId)).toBe(true);
  }
});

test("has no serious accessibility violations", async ({ page }) => {
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
});

test("replays the Hermes W-2 alias and MCP structured-content regressions", async ({ page }) => {
  await page.getByLabel("Type of consent").selectOption("use");
  await page.getByRole("button", { name: "Record client consent (Section 7216)" }).click();
  await expect(page.getByText("Client consent recorded")).toBeVisible();

  await page.getByLabel("Or write your own request").fill(readW2Prompt);
  await page.getByRole("button", { name: "Send to Pru" }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("$48,250");

  await page.getByLabel("Or write your own request").fill(consentStatePrompt);
  await page.getByRole("button", { name: "Send to Pru" }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("consent state was retrieved");
});

test("shows the agent, every traced step, and what Pru now remembers", async ({ page }) => {
  await page.getByLabel("Type of consent").selectOption("use");
  await page.getByRole("button", { name: "Record client consent (Section 7216)" }).click();
  await expect(page.getByText("Client consent recorded")).toBeVisible();
  await expect(page.getByLabel("What Pru remembers about this client")).toContainText("No notes yet.");

  await page.getByRole("button", { name: rememberPrompt }).click();
  const result = page.getByLabel("Assistant result");
  await expect(result).toContainText("Saved: this client prefers callback appointments");
  await expect(result.locator("[data-agent]")).toHaveText(/^Answered by Hermes Agent \S+$/);
  const trace = page.getByRole("list", { name: "Turn trace" });
  // The steps sit folded under the answer until the reader opens them.
  await expect(trace).toBeHidden();
  await result.getByText(/^What happened, step by step/).click();
  await expect(trace).toBeVisible();
  await expect(trace.locator("[data-step='remember_note']")).toContainText("Tool: remember_note");
  await expect(trace.locator("[data-step='remember_note']")).toContainText("ALLOWED");

  const memory = page.getByLabel("What Pru remembers about this client");
  await expect(memory.locator("[data-note]")).toHaveCount(1);
  await expect(memory.locator("[data-note]")).toContainText("Prefers callback appointments on Tuesday mornings.");

  const axe = await new AxeBuilder({ page }).analyze();
  expect(axe.violations).toEqual([]);

  await page.getByRole("button", { name: "Reset demo" }).click();
  await expect(page.getByText("Test data reset")).toBeVisible();
  await expect(memory.locator("[data-note]")).toHaveCount(0);
});

test("shows a recorded turn on load and brings a finished turn into view on a phone", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const recorded = page.getByRole("article", { name: "Recorded turn" });
  await expect(recorded).toContainText("Recorded 2026-10-20 from eval/hermes_turns-2026-10-20.json");

  const consentOnFile = page.locator("[data-consent-on-file]");
  await expect(consentOnFile).toContainText("None.");
  await consentOnFile.getByRole("button", { name: "Record a use consent" }).click();
  await expect(page.getByText("Client consent recorded")).toBeVisible();
  await expect(consentOnFile).toContainText("Use for preparing this return");

  // The first preset is six cells above the result box, so only the page's own scroll can bring
  // the result into view once the turn finishes.
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.getByRole("button", { name: emailPrompt }).click();
  const result = page.getByLabel("Assistant result");
  await expect(result).toBeVisible();
  await expect(recorded).toHaveCount(0);
  // The result's top edge, where its headline sits, must be on screen: a sliver of the box
  // showing at the bottom of the viewport is not enough.
  await expect.poll(() => result.evaluate((element) => {
    const top = element.getBoundingClientRect().top;
    return top >= -1 && top <= window.innerHeight - 120;
  })).toBe(true);
  await expect(result).toBeFocused();
});

for (const width of [390, 768, 1024, 1440]) {
  test(`has no horizontal overflow at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/");
    await expect(page.getByText("Test data ready")).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(1);
  });
}
