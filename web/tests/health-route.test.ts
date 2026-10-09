import { afterEach, describe, expect, it, vi } from "vitest";
import { GET } from "../src/app/healthz/route";

afterEach(() => vi.unstubAllGlobals());

describe("public readiness route", () => {
  it("reports ready only when the gateway reports its local model ready", async () => {
    const gateway = vi.fn().mockResolvedValue(
      new Response('{"status":"ready","gateway":"ready","llama":"ready"}', {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", gateway);

    const response = await GET();

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      status: "ready",
      next: "ready",
      gateway: "ready",
      llama: "ready",
    });
    expect(gateway).toHaveBeenCalledOnce();
    expect(gateway.mock.calls[0]?.[0]).toBe("http://127.0.0.1:8787/healthz");
  });

  it("stays unavailable when llama-server is unavailable", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response('{"status":"starting","gateway":"ready","llama":"unavailable"}', {
          status: 503,
          headers: { "content-type": "application/json" },
        }),
      ),
    );

    const response = await GET();

    expect(response.status).toBe(503);
    expect(await response.json()).toEqual({
      status: "starting",
      next: "ready",
      gateway: "ready",
      llama: "unavailable",
    });
  });
});
