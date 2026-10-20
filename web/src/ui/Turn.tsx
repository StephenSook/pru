import { RECORDED, probeSummary, recordedDate, recordedSteps } from "@/lib/recorded";
import { decisionLabel, reasonText, stepTitle, summarizeTurn, type TraceStep } from "@/lib/turn";

// Where each ledger row's data went, in the gateway's own route names. An agent tool result
// enters the agent's context; it reaches Token Factory only inside a later model call row.
const ROUTES: Record<string, { label: string; tone: string }> = {
  local: { label: "● LOCAL", tone: "route-local" },
  token_factory: { label: "↗ TOKEN FACTORY", tone: "route-cloud" },
  dynamic: { label: "↻ AGENT CONTEXT", tone: "route-agent" },
  dry_run: { label: "◌ DRY RUN", tone: "route-agent" },
  gateway: { label: "● IN PRU", tone: "route-local" },
};

export function RouteBadge({ route }: Readonly<{ route: string }>) {
  const known = ROUTES[route] ?? { label: route.replaceAll("_", " ").toUpperCase(), tone: "route-agent" };
  return <span className={`route-badge ${known.tone}`}>{known.label}</span>;
}

export function TraceList({ steps, label }: Readonly<{ steps: readonly TraceStep[]; label: string }>) {
  return (
    <ol className="step-list" aria-label={label}>
      {steps.map((step) => (
        <li className="step" key={step.key} data-step={step.toolName ?? step.kind}>
          <div>
            <RouteBadge route={step.route} />
            <strong>{decisionLabel(step.decision)}</strong>
            <span className="step-title">{stepTitle(step)}</span>
            {step.dryRun ? <span className="dry-run">DRY RUN, NOTHING SENT</span> : null}
          </div>
          <p>{reasonText(step.reason)} <span className="reason-code">({step.reason})</span></p>
          {step.redactedSpans ? (
            <p className="redacted">{step.redactedSpans} sensitive {step.redactedSpans === 1 ? "span was" : "spans were"} removed before this call.</p>
          ) : null}
          <dl>
            {step.modelId !== null ? <div><dt>Model</dt><dd>{step.modelId}</dd></div> : null}
            <div><dt>Latency</dt><dd>{step.latencyMs} ms</dd></div>
            <div><dt>Tokens</dt><dd>{step.inputTokens} in / {step.outputTokens} out</dd></div>
            {step.costUsd !== null ? <div><dt>Cost</dt><dd>${step.costUsd.toFixed(8)}</dd></div> : null}
          </dl>
        </li>
      ))}
    </ol>
  );
}

// A finished turn from the committed receipt, shown before any click. Every value on it is read
// from web/src/data/recorded-turn.json, which tests compare field by field with the receipt.
export function RecordedTurn() {
  const steps = recordedSteps();
  const summary = summarizeTurn(steps);
  const probes = probeSummary();
  const recipient = RECORDED.eval_setup.disclose_recipient;
  const drafted = summary.dryRunTools.includes("draft_client_email");
  return (
    <article className="answer recorded" aria-label="Recorded turn">
      <p className="recorded-source" data-recorded-source>
        Recorded {recordedDate()} from {RECORDED.source}
      </p>
      <p className="recorded-request">Request: {RECORDED.prompt.text}</p>
      <h3 className="turn-headline" data-recorded-headline>
        {summary.decision === "ALLOWED" && drafted
          ? `Drafted, not sent: the W-2 email to ${recipient}`
          : `${summary.decision}: ${RECORDED.prompt.text}`}
      </h3>
      <p className="recorded-why">
        Before this request the eval recorded a use consent and a disclose consent naming {recipient}{" "}
        ({RECORDED.eval_setup.source}). Without the disclose consent the gateway refuses the draft.
      </p>
      <p className="recorded-answer" data-recorded-answer>{RECORDED.turn.answer}</p>
      <ul className="recorded-facts">
        <li data-recorded-ssn>
          {RECORDED.turn.ssn_reached_token_factory ? "An SSN reached Token Factory." : "No SSN reached Token Factory."}
          {summary.redactedSpans > 0 ? ` ${summary.redactedSpans} SSN ${summary.redactedSpans === 1 ? "span was" : "spans were"} removed from the W-2 before the agent read it.` : ""}
        </li>
        <li data-recorded-probes>
          In the same run, {probes.denied} of {probes.total} attempts to send an SSN to Token Factory were denied ({probes.reasons.join(", ")}).
        </li>
      </ul>
      <TraceList steps={steps} label="Recorded turn trace" />
      <p className="recorded-meta" data-recorded-meta>
        {RECORDED.agent} with {RECORDED.model}, run {RECORDED.turn.run} of this prompt. {RECORDED.turn.latency_ms.wall} ms end to end.
        Estimated cost ${RECORDED.turn.estimated_cost_usd} ({RECORDED.cost_basis}).
      </p>
    </article>
  );
}
