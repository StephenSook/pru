import { afterEach, describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";
import { GET, POST } from "../src/app/api/pru/[...path]/route";

type StreamEvent = {
  type: string;
  status?: number;
  body?: string;
};

async function streamEvents(response: Response): Promise<StreamEvent[]> {
  return (await response.text())
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as StreamEvent);
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

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

  it("forwards the per-client notes route", async () => {
    const gateway = vi.fn().mockResolvedValue(new Response(
      '[{"id":"11111111-1111-4111-8111-111111111111","text":"Use TEST DATA only","created_at":"2026-10-19T12:00:00Z","ledger_entry_id":"22222222-2222-4222-8222-222222222222"}]',
      { status: 200, headers: { "content-type": "application/json" } },
    ));
    vi.stubGlobal("fetch", gateway);

    const response = await GET(
      new NextRequest("http://judge.test/api/pru/v1/clients/demo-avery/notes"),
      { params: Promise.resolve({ path: ["v1", "clients", "demo-avery", "notes"] }) },
    );

    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject([{ text: "Use TEST DATA only" }]);
    expect(gateway).toHaveBeenCalledOnce();
    expect(gateway.mock.calls[0]?.[0]).toBe(
      "http://127.0.0.1:8787/v1/clients/demo-avery/notes",
    );
  });

  it("preserves gateway status inside the chat stream and sends only JSON upstream", async () => {
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

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("application/x-ndjson");
    const events = await streamEvents(response);
    expect(events[0]?.type).toBe("progress");
    expect(events.at(-1)).toMatchObject({ type: "result", status: 403 });
    expect(JSON.parse(events.at(-1)?.body ?? "{}")).toEqual({ error: "policy denied" });
    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    const headers = new Headers(init.headers);
    expect(headers.get("content-type")).toBe("application/json");
    expect(headers.get("x-pru-workspace-id")).toBe("11111111-1111-4111-8111-111111111111");
    expect(headers.get("x-pru-workspace-id")).not.toBe("attacker-value");
  });

  it("streams progress before the gateway produces a chat response", async () => {
    let resolveGateway: ((response: Response) => void) | undefined;
    const gateway = vi.fn().mockImplementation(() => new Promise<Response>((resolve) => {
      resolveGateway = resolve;
    }));
    vi.stubGlobal("fetch", gateway);

    const response = await POST(
      new NextRequest("http://judge.test/api/pru/v1/clients/demo-avery/chat", {
        method: "POST",
        body: JSON.stringify({ message: "What is the test SSN?" }),
      }),
      { params: Promise.resolve({ path: ["v1", "clients", "demo-avery", "chat"] }) },
    );
    const reader = response.body?.getReader();
    expect(reader).toBeDefined();
    const first = await reader!.read();
    const firstEvent = JSON.parse(new TextDecoder().decode(first.value).trim()) as StreamEvent;
    expect(firstEvent.type).toBe("progress");
    expect(resolveGateway).toBeTypeOf("function");

    resolveGateway!(new Response('{"answer":"done"}', {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    let remaining = "";
    while (true) {
      const next = await reader!.read();
      if (next.done) break;
      remaining += new TextDecoder().decode(next.value);
    }
    const finalEvent = JSON.parse(remaining.trim()) as StreamEvent;
    expect(finalEvent).toMatchObject({ type: "result", status: 200 });
    expect(response.headers.get("x-accel-buffering")).toBe("no");
    expect(response.headers.get("cache-control")).toContain("no-transform");
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

  it("selects a trusted hop from the right of an appended forwarding chain", async () => {
    vi.stubEnv("PRU_TRUSTED_CLIENT_IP_HEADER", "x-forwarded-for");
    vi.stubEnv("PRU_TRUSTED_PROXY_HOPS", "1");
    const gateway = vi.fn().mockResolvedValue(new Response("[]", { status: 200 }));
    vi.stubGlobal("fetch", gateway);

    await GET(
      new NextRequest("http://judge.test/api/pru/v1/demo/clients", {
        headers: {
          "x-forwarded-for": "198.51.100.99, 203.0.113.10",
        },
      }),
      { params: Promise.resolve({ path: ["v1", "demo", "clients"] }) },
    );

    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).get("x-pru-client-ip")).toBe("203.0.113.10");
  });

  it("fails closed when the trusted proxy hop count is invalid", async () => {
    vi.stubEnv("PRU_TRUSTED_CLIENT_IP_HEADER", "x-forwarded-for");
    vi.stubEnv("PRU_TRUSTED_PROXY_HOPS", "2");
    const gateway = vi.fn().mockResolvedValue(new Response("[]", { status: 200 }));
    vi.stubGlobal("fetch", gateway);

    await GET(
      new NextRequest("http://judge.test/api/pru/v1/demo/clients", {
        headers: { "x-forwarded-for": "203.0.113.10" },
      }),
      { params: Promise.resolve({ path: ["v1", "demo", "clients"] }) },
    );

    const [, init] = gateway.mock.calls[0] as [string, RequestInit];
    expect(new Headers(init.headers).get("x-pru-client-ip")).toBe("unknown");
  });
});
