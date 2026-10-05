import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 180_000,
  expect: { timeout: 120_000 },
  use: {
    baseURL: process.env.PRU_WEB_URL ?? "http://127.0.0.1:3000",
    trace: "retain-on-failure",
  },
});
