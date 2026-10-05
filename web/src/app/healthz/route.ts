import { NextResponse } from "next/server";

export async function GET() {
  try {
    const response = await fetch(`${process.env.PRU_GATEWAY_URL ?? "http://127.0.0.1:8787"}/healthz`, {
      signal: AbortSignal.timeout(2_000),
      cache: "no-store",
    });
    if (!response.ok) throw new Error("gateway unhealthy");
    return NextResponse.json({ status: "ok" });
  } catch {
    return NextResponse.json({ status: "unhealthy" }, { status: 503 });
  }
}
