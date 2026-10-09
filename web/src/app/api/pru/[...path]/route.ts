import { randomUUID } from "node:crypto";
import { isIP } from "node:net";
import { NextRequest, NextResponse } from "next/server";

const GATEWAY_URL = process.env.PRU_GATEWAY_URL ?? "http://127.0.0.1:8787";
const MAX_BODY_BYTES = 16_384;
export const WORKSPACE_COOKIE = "pru_workspace";
export const WORKSPACE_IDLE_TTL_SECONDS = 3_600;
const WORKSPACE_HEADER = "x-pru-workspace-id";
const CLIENT_IP_HEADER = "x-pru-client-ip";
const WORKSPACE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const CLIENT = "[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}";
const ID = "[0-9a-fA-F-]{36}";
const ALLOWED = [
  /^v1\/demo\/(clients|reset)$/,
  new RegExp(`^v1/clients/${CLIENT}/(chat|ledger|consents)$`),
  new RegExp(`^v1/clients/${CLIENT}/consents/${ID}/revoke$`),
];

function safePath(parts: string[]): string | null {
  const path = parts.join("/");
  return ALLOWED.some((pattern) => pattern.test(path)) ? path : null;
}

function workspaceId(request: NextRequest): string {
  const existing = request.cookies.get(WORKSPACE_COOKIE)?.value;
  return existing && WORKSPACE_ID.test(existing) ? existing : randomUUID();
}

function clientIp(request: NextRequest): string {
  const trustedHeader = process.env.PRU_TRUSTED_CLIENT_IP_HEADER?.trim().toLowerCase();
  if (!trustedHeader) return "unknown";
  const candidate = request.headers.get(trustedHeader)?.split(",", 1)[0]?.trim() ?? "";
  return isIP(candidate) ? candidate : "unknown";
}

function forWorkspace(response: NextResponse, request: NextRequest, id: string): NextResponse {
  response.headers.set("cache-control", "private, no-store");
  response.headers.set("vary", "Cookie");
  response.cookies.set(WORKSPACE_COOKIE, id, {
    httpOnly: true,
    sameSite: "lax",
    secure: request.nextUrl.protocol === "https:",
    path: "/",
    maxAge: WORKSPACE_IDLE_TTL_SECONDS,
  });
  return response;
}

async function proxy(request: NextRequest, context: { params: Promise<{ path: string[] }> }) {
  const { path: parts } = await context.params;
  const path = safePath(parts);
  if (!path) {
    return NextResponse.json({ error: "unsupported gateway path" }, { status: 404 });
  }
  const id = workspaceId(request);

  let body: string | undefined;
  if (request.method === "POST") {
    body = await request.text();
    if (new TextEncoder().encode(body).byteLength > MAX_BODY_BYTES) {
      return forWorkspace(
        NextResponse.json({ error: "request body too large" }, { status: 413 }),
        request,
        id,
      );
    }
  }

  try {
    const headers = new Headers({
      [WORKSPACE_HEADER]: id,
      [CLIENT_IP_HEADER]: clientIp(request),
    });
    if (body) headers.set("content-type", "application/json");
    const response = await fetch(`${GATEWAY_URL}/${path}`, {
      method: request.method,
      body,
      headers,
      signal: AbortSignal.timeout(300_000),
      cache: "no-store",
    });
    const responseBody = await response.text();
    return forWorkspace(
      new NextResponse(responseBody, {
        status: response.status,
        headers: { "content-type": response.headers.get("content-type") ?? "application/json" },
      }),
      request,
      id,
    );
  } catch {
    return forWorkspace(
      NextResponse.json({ error: "gateway unavailable" }, { status: 502 }),
      request,
      id,
    );
  }
}

export const GET = proxy;
export const POST = proxy;
