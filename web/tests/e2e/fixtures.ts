import { expect, test as base } from "@playwright/test";

// The gateway limits each client address to 120 requests a minute. Every test here would otherwise
// share one address, so each page gets its own, the way each real visitor has one. The e2e scripts
// start the web server with PRU_TRUSTED_CLIENT_IP_HEADER=x-forwarded-for so this header is honored.
let next = 0;

export const test = base.extend({
  page: async ({ page }, provide, testInfo) => {
    next += 1;
    await page.setExtraHTTPHeaders({ "x-forwarded-for": `10.0.${testInfo.workerIndex % 250}.${(next % 250) + 1}` });
    await provide(page);
  },
});

export { expect };
