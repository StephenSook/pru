import { NextResponse } from "next/server";

export async function GET() {
  try {
    const response = await fetch(`${process.env.PRU_GATEWAY_URL ?? "http://127.0.0.1:8787"}/healthz`, {
      signal: AbortSignal.timeout(2_000),
      cache: "no-store",
    });
    const gateway = (await response.json()) as {
      status?: string;
      gateway?: string;
      llama?: string;
    };
    if (!response.ok) {
      return NextResponse.json(
        {
          status: "starting",
          next: "ready",
          gateway: gateway.gateway ?? "unavailable",
          llama: gateway.llama ?? "unavailable",
        },
        { status: 503, headers: { "cache-control": "no-store" } },
      );
    }
    return NextResponse.json(
      { status: "ready", next: "ready", gateway: "ready", llama: "ready" },
      { headers: { "cache-control": "no-store" } },
    );
  } catch {
    return NextResponse.json(
      { status: "starting", next: "ready", gateway: "unavailable", llama: "unknown" },
      { status: 503, headers: { "cache-control": "no-store" } },
    );
  }
}
