import type { Ref } from "react";
import type { ChatResponse } from "@/lib/contracts";
import { RECORDED, probeSummary, recordedDate, recordedSteps } from "@/lib/recorded";
import {
  decisionLabel,
  fromChatStep,
  reasonText,
  stepTitle,
  summarizeTurn,
  verdictSentence,
  type TraceStep,
  type TurnSummary,
} from "@/lib/turn";

// Where each ledger row's data went, in the gateway's own route names. An agent tool result
// enters the agent's context; it reaches Token Factory only inside a later model call row.
const ROUTES: Record<string, { label: string; tone: string }> = {
  local: { label: "Local model", tone: "route-local" },
  token_factory: { label: "Token Factory", tone: "route-cloud" },
  dynamic: { label: "Agent's working context", tone: "route-agent" },
  dry_run: { label: "Dry run", tone: "route-agent" },
  gateway: { label: "Inside Pru", tone: "route-local" },
};

export function RouteBadge({ route }: Readonly<{ route: string }>) {
  const known = ROUTES[route] ?? { label: route.replaceAll("_", " "), tone: "route-agent" };
  return <span className={`route-badge ${known.tone}`}>{known.label}</span>;
}

export function TraceList({ steps, label }: Readonly<{ steps: readonly TraceStep[]; label: string }>) {
  return (
    <ol className="step-list" aria-label={label}>
      {steps.map((step, index) => (
        <li className="step" key={step.key} data-step={step.toolName ?? step.kind}>
          <div>
            <span className="line-number" aria-hidden="true">{index + 1}</span>
            <RouteBadge route={step.route} />
            <strong className={`decision decision-${step.decision}`}>{decisionLabel(step.decision)}</strong>
            <span className="step-title">{stepTitle(step)}</span>
            {step.dryRun ? <span className="dry-run">Nothing sent</span> : null}
          </div>
          <p>{reasonText(step.reason, step.decision)} <span className="reason-code">({step.reason})</span></p>
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

// The evidence sits below the answer, folded: the decision is the headline, the steps are proof.
function TraceDetails({ steps, label }: Readonly<{ steps: readonly TraceStep[]; label: string }>) {
  return (
    <details className="trace">
      <summary>What happened, step by step ({steps.length} {steps.length === 1 ? "step" : "steps"})</summary>
      <TraceList steps={steps} label={label} />
    </details>
  );
}

// The first large line of every turn: the decision as a stamp, then where the data went.
export function TurnVerdict({ summary, stampKey }: Readonly<{ summary: TurnSummary; stampKey?: string }>) {
  const deny = summary.decision === "DENIED";
  return (
    <h3 className="verdict" data-decision={summary.decision} data-routes={summary.routes.join(",")}>
      <span key={stampKey} className={`stamp ${deny ? "stamp-deny" : "stamp-allow"}${stampKey ? " stamp-live" : ""}`}>
        {summary.decision}
      </span>
      {summary.routes.length > 0 ? (
        <span className="ran-on">
          <span className="ran-on-label">{deny ? "model calls ran on" : "ran on"}</span>
          {summary.routes.map((route) => (
            <span key={route} className={`route-stamp${route === "LOCAL" ? " route-stamp-local" : ""}`} data-route-stamp={route}>
              {route}
            </span>
          ))}
        </span>
      ) : (
        <span className="ran-on ran-on-label">nothing was sent to a model</span>
      )}
    </h3>
  );
}

export function LiveTurn({ turn, resultRef }: Readonly<{ turn: ChatResponse; resultRef?: Ref<HTMLElement> }>) {
  const steps = turn.steps.map(fromChatStep);
  const summary = summarizeTurn(steps);
  return (
    <article className="answer" aria-label="Assistant result" ref={resultRef} tabIndex={-1}>
      {turn.agent ? <p className="agent-line" data-agent>Answered by {turn.agent.name} {turn.agent.version}</p> : null}
      <TurnVerdict summary={summary} stampKey={turn.turn_id} />
      <p className="verdict-sentence">{verdictSentence(summary)}</p>
      <p className="answer-prose">{turn.answer}</p>
      <TraceDetails steps={steps} label="Turn trace" />
    </article>
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
      <TurnVerdict summary={summary} />
      <p className="turn-headline" data-recorded-headline>
        {summary.decision === "ALLOWED" && drafted
          ? `Drafted, not sent: the W-2 email to ${recipient}`
          : verdictSentence(summary)}
      </p>
      <p className="recorded-answer" data-recorded-answer>{RECORDED.turn.answer}</p>
      <ul className="recorded-facts">
        <li>
          Before this request the eval recorded a use consent and a disclose consent naming {recipient}{" "}
          ({RECORDED.eval_setup.source}). Without the disclose consent the gateway refuses the draft.
        </li>
        <li data-recorded-ssn>
          {RECORDED.turn.ssn_reached_token_factory ? "An SSN reached Token Factory." : "No SSN reached Token Factory."}
          {summary.redactedSpans > 0 ? ` ${summary.redactedSpans} SSN ${summary.redactedSpans === 1 ? "span was" : "spans were"} removed from the W-2 before the agent read it.` : ""}
        </li>
        <li data-recorded-probes>
          In the same run, {probes.denied} of {probes.total} attempts to send an SSN to Token Factory were denied ({probes.reasons.join(", ")}).
        </li>
      </ul>
      <TraceDetails steps={steps} label="Recorded turn trace" />
      <p className="recorded-meta" data-recorded-meta>
        {RECORDED.agent} with {RECORDED.model}, run {RECORDED.turn.run} of this prompt. {RECORDED.turn.latency_ms.wall} ms end to end.
        Estimated cost ${RECORDED.turn.estimated_cost_usd} ({RECORDED.cost_basis}).
      </p>
    </article>
  );
}
