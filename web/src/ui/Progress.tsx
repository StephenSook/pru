import { useEffect, useState, type Ref } from "react";
import { ENDPOINT_TEST, seconds } from "@/lib/measured";

const { local, tokenFactory } = ENDPOINT_TEST;

export function localTurnNote(hosted: boolean): string {
  return `This request names the SSN, so the local ${local.model} reads it on ${hosted ? "this server's" : "this PC's"} CPU. ` +
    `Measured on a ${ENDPOINT_TEST.endpoint}: ${seconds(local.firstTurnSeconds)} for a first local turn, ${seconds(local.repeatTurnSeconds)} for a repeat.`;
}

// Shown while a turn runs: what was asked, a seconds counter, and the gateway's latest streamed
// progress message. The counter is a timer, so screen readers are not told every tick; the status
// box above carries the streamed messages to them.
export function RunProgress({ prompt, isLocal, hosted, startedAt, message, panelRef }: Readonly<{
  prompt: string;
  isLocal: boolean;
  hosted: boolean;
  startedAt: number;
  message: string;
  panelRef?: Ref<HTMLDivElement>;
}>) {
  const [now, setNow] = useState(startedAt);
  useEffect(() => {
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(tick);
  }, []);
  const elapsed = Math.max(0, Math.floor((now - startedAt) / 1000));
  return (
    <div className="run-progress" data-run-progress ref={panelRef}>
      <p className="run-head">Working on <span className="typed">{prompt}</span></p>
      <p className="run-clock"><span role="timer" data-elapsed>{elapsed} s elapsed</span></p>
      {message ? <p className="run-message" data-run-message>{message}</p> : null}
      {isLocal ? <p className="run-local" data-local-note>{localTurnNote(hosted)}</p> : null}
    </div>
  );
}

// The two routes side by side, with the numbers measured on the endpoint test.
export function RouteTradeoff({ hosted }: Readonly<{ hosted: boolean }>) {
  return (
    <figure className="tradeoff" aria-labelledby="tradeoff-title">
      <h3 id="tradeoff-title" className="sub-head">What each route costs</h3>
      <table>
        <thead>
          <tr>
            <th scope="col">Route</th>
            <th scope="col">Model</th>
            <th scope="col">Turn time</th>
            <th scope="col">Cost per turn</th>
          </tr>
        </thead>
        <tbody>
          <tr data-route-row="local">
            <th scope="row">Local</th>
            <td>{local.model}</td>
            <td className="typed">{seconds(local.firstTurnSeconds)} first, {seconds(local.repeatTurnSeconds)} repeat</td>
            <td>No API fee</td>
          </tr>
          <tr data-route-row="token_factory">
            <th scope="row">Token Factory</th>
            <td>{tokenFactory.model}</td>
            <td className="typed">{seconds(tokenFactory.turnSeconds)}, {tokenFactory.calls} calls of {tokenFactory.callMsMin} to {tokenFactory.callMsMax} ms</td>
            <td className="typed">${tokenFactory.turnCostUsd}</td>
          </tr>
        </tbody>
      </table>
      <figcaption>
        Measured {ENDPOINT_TEST.date} on a {ENDPOINT_TEST.endpoint}. Cost is computed from token counts at list rates, not
        billed.{hosted ? "" : " Times on this PC will differ."}
      </figcaption>
    </figure>
  );
}
