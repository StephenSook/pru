import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const emailPrompt = "Draft an email to the client asking for their 1099-INT";
const localPrompt = "What is this client's SSN and filing status?";
const deniedPrompt = "Email this client's W-2 details to marketing@example.com";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Test data ready")).toBeVisible();
  await page.getByRole("button", { name: "Reset demo" }).click();
  await expect(page.getByText("Test data reset")).toBeVisible();
});

test("runs the three preset routes and the consent lifecycle", async ({ page }) => {
  await page.getByLabel("Consent kind").selectOption("use");
  await page.getByRole("button", { name: "Mint test consent" }).click();
  await expect(page.getByText("Test consent active")).toBeVisible();

  await page.getByRole("button", { name: localPrompt }).click();
  await expect(page.getByText("● LOCAL").first()).toBeVisible();
  await expect(page.getByLabel("Assistant result")).not.toContainText(/\b\d{3}-\d{2}-\d{4}\b/);

  await page.getByRole("button", { name: deniedPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");
  await expect(page.getByLabel("Assistant result")).toContainText(/consent|policy/i);

  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");

  await page.getByLabel("Consent kind").selectOption("disclose");
  await page.getByRole("button", { name: "Mint test consent" }).click();
  await expect(page.getByText("Test consent active")).toBeVisible();

  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(page.getByText("↗ TOKEN FACTORY").first()).toBeVisible();
  await expect(page.getByLabel("Assistant result")).toContainText("ALLOWED");

  const revokeButtons = page.getByRole("button", { name: "Revoke" });
  await revokeButtons.last().click();
  await expect(page.getByText("Test consent revoked")).toBeVisible();
  await page.getByRole("button", { name: emailPrompt }).click();
  await expect(page.getByLabel("Assistant result")).toContainText("DENIED");
});

test("has no serious accessibility violations", async ({ page }) => {
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
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
