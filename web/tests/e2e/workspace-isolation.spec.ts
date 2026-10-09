import { expect, test } from "@playwright/test";

const COOKIE = "pru_workspace";

test("keeps two browser workspaces separate and resets only the caller", async ({ browser }) => {
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  try {
    const pageA = await contextA.newPage();
    const pageB = await contextB.newPage();
    await Promise.all([pageA.goto("/"), pageB.goto("/")]);
    await Promise.all([
      expect(pageA.getByText("Test data ready")).toBeVisible(),
      expect(pageB.getByText("Test data ready")).toBeVisible(),
    ]);

    const cookieA = (await contextA.cookies()).find((cookie) => cookie.name === COOKIE);
    const cookieB = (await contextB.cookies()).find((cookie) => cookie.name === COOKIE);
    expect(cookieA).toBeDefined();
    expect(cookieB).toBeDefined();
    expect(cookieA?.httpOnly).toBe(true);
    expect(cookieB?.httpOnly).toBe(true);
    expect(cookieA?.sameSite).toBe("Lax");
    expect(cookieB?.sameSite).toBe("Lax");
    expect(cookieA?.value).not.toBe(cookieB?.value);
    expect(await pageA.evaluate(() => document.cookie)).not.toContain(COOKIE);
    expect(await pageB.evaluate(() => document.cookie)).not.toContain(COOKIE);

    await pageA.getByRole("button", { name: "Mint test consent" }).click();
    await pageB.getByRole("button", { name: "Mint test consent" }).click();
    await Promise.all([
      expect(pageA.getByText("Test consent active")).toBeVisible(),
      expect(pageB.getByText("Test consent active")).toBeVisible(),
    ]);

    await pageA.getByRole("button", { name: "Reset demo" }).click();
    await expect(pageA.getByText("Test data reset")).toBeVisible();
    await expect(pageA.getByText("No consent exists for this client.")).toBeVisible();
    await pageB.reload();
    await expect(pageB.getByText("Test data ready")).toBeVisible();
    await expect(pageB.getByText("ACTIVE")).toBeVisible();
  } finally {
    await contextA.close();
    await contextB.close();
  }
});
