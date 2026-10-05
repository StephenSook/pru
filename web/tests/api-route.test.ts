import { afterEach, describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";
import { GET, POST } from "../src/app/api/pru/[...path]/route";

afterEach(() => vi.unstubAllGlobals());

describe("server-side gateway route", () => {
  it("forwards an allowed request without browser credentials", async () => {
    const gateway = vi.fn().mockResolvedValue(new Response('[{"id":"demo-avery"}]', {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", gateway);

    const response = await GET(
      new NextRequest("http://judge.test/api/pru/v1/demo/clients", {
        headers: { authorization: "Bearer browser-secret", cookie: "session=browser-secret" },
      }),
      { params: Promise.resolve({ path: ["v1", "demo", "clients"] }) },
    );

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual([{ id: "demo-avery" }]);
    expect(gateway).toHaveBeenCalledOnce();
    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    expect(init.headers).toBeUndefined();
    expect(JSON.stringify(init)).not.toContain("browser-secret");
  });

  it("rejects paths outside the fixed gateway surface", async () => {
    const gateway = vi.fn();
    vi.stubGlobal("fetch", gateway);

    const response = await GET(
      new NextRequest("http://judge.test/api/pru/admin/secrets"),
      { params: Promise.resolve({ path: ["admin", "secrets"] }) },
    );

    expect(response.status).toBe(404);
    expect(gateway).not.toHaveBeenCalled();
  });

  it("preserves gateway status and sends only JSON content", async () => {
    const gateway = vi.fn().mockResolvedValue(new Response('{"error":"policy denied"}', {
      status: 403,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", gateway);

    const response = await POST(
      new NextRequest("http://judge.test/api/pru/v1/clients/demo-avery/chat", {
        method: "POST",
        body: JSON.stringify({ message: "hello" }),
      }),
      { params: Promise.resolve({ path: ["v1", "clients", "demo-avery", "chat"] }) },
    );

    expect(response.status).toBe(403);
    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    expect(init.headers).toEqual({ "content-type": "application/json" });
  });
});
