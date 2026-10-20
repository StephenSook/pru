// Route trade-offs measured on 2026-10-20 in a Nebius Serverless endpoint test: one cpu-d3
// endpoint (4 vCPU, 16 GB RAM) running this repository's container image. The figures were
// recorded by hand during that test; no receipt file for it is committed. The Token Factory cost
// is computed from returned token counts at list rates, not read from a bill. Times on another
// machine, including a PC running the test build, will differ.
export const ENDPOINT_TEST = {
  date: "2026-10-20",
  endpoint: "Nebius cpu-d3 endpoint, 4 vCPU",
  local: {
    model: "Nemotron 3 Nano 4B",
    firstTurnSeconds: 11.1,
    repeatTurnSeconds: 5.0,
  },
  tokenFactory: {
    model: "Nemotron 3.5 Lightning",
    turnSeconds: 2.9,
    calls: 3,
    callMsMin: 315,
    callMsMax: 640,
    turnCostUsd: 0.00046,
  },
} as const;

export function seconds(value: number): string {
  return `${value.toFixed(1)} s`;
}
