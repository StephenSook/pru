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
    const headers = new Headers(init.headers);
    expect(headers.get("x-pru-workspace-id")).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(headers.get("x-pru-client-ip")).toBe("unknown");
    expect(headers.get("authorization")).toBeNull();
    expect(headers.get("cookie")).toBeNull();
    expect(JSON.stringify(init)).not.toContain("browser-secret");
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    const cookie = response.headers.get("set-cookie") ?? "";
    expect(cookie).toContain("pru_workspace=");
    expect(cookie).toContain("HttpOnly");
    expect(cookie).toContain("SameSite=lax");
    expect(cookie).toContain("Max-Age=3600");
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
        headers: {
          cookie: "pru_workspace=11111111-1111-4111-8111-111111111111",
          "x-pru-workspace-id": "attacker-value",
        },
      }),
      { params: Promise.resolve({ path: ["v1", "clients", "demo-avery", "chat"] }) },
    );

    expect(response.status).toBe(403);
    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    const headers = new Headers(init.headers);
    expect(headers.get("content-type")).toBe("application/json");
    expect(headers.get("x-pru-workspace-id")).toBe("11111111-1111-4111-8111-111111111111");
    expect(headers.get("x-pru-workspace-id")).not.toBe("attacker-value");
  });

  it("forwards only a validated address from the configured ingress header", async () => {
    vi.stubEnv("PRU_TRUSTED_CLIENT_IP_HEADER", "x-host-client-ip");
    const gateway = vi.fn().mockResolvedValue(new Response("[]", { status: 200 }));
    vi.stubGlobal("fetch", gateway);

    await GET(
      new NextRequest("http://judge.test/api/pru/v1/demo/clients", {
        headers: {
          "x-host-client-ip": "203.0.113.10",
          "x-pru-client-ip": "198.51.100.99",
        },
      }),
      { params: Promise.resolve({ path: ["v1", "demo", "clients"] }) },
    );

    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).get("x-pru-client-ip")).toBe("203.0.113.10");
  });
});
