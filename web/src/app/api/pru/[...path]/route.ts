import { NextRequest, NextResponse } from "next/server";

const GATEWAY_URL = process.env.PRU_GATEWAY_URL ?? "http://127.0.0.1:8787";
const MAX_BODY_BYTES = 16_384;
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

async function proxy(request: NextRequest, context: { params: Promise<{ path: string[] }> }) {
  const { path: parts } = await context.params;
  const path = safePath(parts);
  if (!path) {
    return NextResponse.json({ error: "unsupported gateway path" }, { status: 404 });
  }

  let body: string | undefined;
  if (request.method === "POST") {
    body = await request.text();
    if (new TextEncoder().encode(body).byteLength > MAX_BODY_BYTES) {
      return NextResponse.json({ error: "request body too large" }, { status: 413 });
    }
  }

  try {
    const response = await fetch(`${GATEWAY_URL}/${path}`, {
      method: request.method,
      body,
      headers: body ? { "content-type": "application/json" } : undefined,
      signal: AbortSignal.timeout(300_000),
      cache: "no-store",
    });
    const responseBody = await response.text();
    return new NextResponse(responseBody, {
      status: response.status,
      headers: { "content-type": response.headers.get("content-type") ?? "application/json" },
    });
  } catch {
    return NextResponse.json({ error: "gateway unavailable" }, { status: 502 });
  }
}

export const GET = proxy;
export const POST = proxy;
