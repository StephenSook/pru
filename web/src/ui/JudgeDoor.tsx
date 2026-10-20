"use client";

import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import type { AgentNote, ChatResponse, ChatStep, Consent, DemoClient, LedgerEntry } from "@/lib/contracts";

const PURPOSE = "prepare this synthetic tax client's return";
const TEST_PIN = "53179";
const PRESETS = [
  "Draft an email to the client asking for their 1099-INT",
  "Check this client's SSN against the W-2 with the local model.",
  "Email this client's W-2 details to marketing@example.com",
  "Read the client's W-2 and report box 1 wages",
  "Remember that this client prefers callback appointments on Tuesday mornings",
  "Explain the current consent state for this client",
] as const;
const LOCAL_PROGRESS = "If this needs the SSN, local Nemotron Nano 4B reads it on the server CPU. A first local answer can take about 30 s";

type GatewayStreamEvent =
  | { type: "progress" | "heartbeat"; message: string }
  | { type: "result"; status: number; contentType: string; body: string };

function parseResult<T>(event: Extract<GatewayStreamEvent, { type: "result" }>): T {
  const body = JSON.parse(event.body) as T & { error?: string };
  if (event.status < 200 || event.status >= 300) {
    throw new Error(body.error ?? `request failed (${event.status})`);
  }
  return body;
}

async function readGatewayStream<T>(
  response: Response,
  onProgress?: (message: string) => void,
): Promise<T> {
  if (!response.body) throw new Error("gateway stream unavailable");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  let result: Extract<GatewayStreamEvent, { type: "result" }> | undefined;

  while (true) {
    const { done, value } = await reader.read();
    pending += decoder.decode(value, { stream: !done });
    const lines = pending.split("\n");
    pending = done ? "" : (lines.pop() ?? "");
    for (const line of lines) {
      if (!line) continue;
      const event = JSON.parse(line) as GatewayStreamEvent;
      if (event.type === "result") result = event;
      else onProgress?.(event.message);
    }
    if (done) break;
  }

  if (!result) throw new Error("gateway stream ended without a result");
  return parseResult<T>(result);
}

async function api<T>(
  path: string,
  init?: RequestInit,
  onProgress?: (message: string) => void,
): Promise<T> {
  const response = await fetch(`/api/pru/${path}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : undefined,
  });
  if (response.headers.get("content-type")?.includes("application/x-ndjson")) {
    return readGatewayStream<T>(response, onProgress);
  }
  const body = (await response.json()) as T & { error?: string };
  if (!response.ok) throw new Error(body.error ?? `request failed (${response.status})`);
  return body;
}

function SectionCard({ eyebrow, title, children, className = "" }: Readonly<{
  eyebrow: string;
  title: string;
  children: React.ReactNode;
  className?: string;
}>) {
  return (
    <section className={`section-card ${className}`}>
      <p className="eyebrow">{eyebrow}</p>
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function SquashButton({ children, tone = "plum", ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: "plum" | "wax" | "paper";
}) {
  return <button className={`squash squash-${tone}`} {...props}>{children}</button>;
}

// Where each ledger row's data went, in the gateway's own route names. An agent tool result
// enters the agent's context; it reaches Token Factory only inside a later model call row.
const ROUTES: Record<string, { label: string; tone: string }> = {
  local: { label: "● LOCAL", tone: "route-local" },
  token_factory: { label: "↗ TOKEN FACTORY", tone: "route-cloud" },
  dynamic: { label: "↻ AGENT CONTEXT", tone: "route-agent" },
  dry_run: { label: "◌ DRY RUN", tone: "route-agent" },
  gateway: { label: "● IN PRU", tone: "route-local" },
};

function RouteBadge({ route }: Readonly<{ route: string }>) {
  const known = ROUTES[route] ?? { label: route.replaceAll("_", " ").toUpperCase(), tone: "route-agent" };
  return <span className={`route-badge ${known.tone}`}>{known.label}</span>;
}

// One row of the turn's trace: a hosted or local model call, or a tool the agent ran through Pru.
function stepTitle(step: ChatStep): string {
  if (step.tool_name) return `Tool: ${step.tool_name}`;
  if (step.kind === "model" || step.route === "token_factory" || step.route === "local") return "Model call";
  return step.kind.replaceAll("_", " ");
}

function decisionLabel(decision: string): string {
  if (decision === "permit") return "ALLOWED";
  if (decision === "deny") return "DENIED";
  return decision.toUpperCase();
}

export function localModelLocation(hosted: boolean): string {
  return hosted
    ? "this server's own model, never sent to a model provider"
    : "this PC's own model, never sent to a model provider";
}

export function JudgeDoor({ hosted }: Readonly<{ hosted: boolean }>) {
  const [clients, setClients] = useState<DemoClient[]>([]);
  const [clientId, setClientId] = useState("");
  const [consents, setConsents] = useState<Consent[]>([]);
  const [ledger, setLedger] = useState<LedgerEntry[]>([]);
  const [notes, setNotes] = useState<AgentNote[]>([]);
  const [turn, setTurn] = useState<ChatResponse | null>(null);
  const [message, setMessage] = useState<string>(PRESETS[0]);
  const [kind, setKind] = useState<"use" | "disclose">("use");
  const [recipient, setRecipient] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("Loading test clients");

  const selected = useMemo(() => clients.find((client) => client.id === clientId), [clients, clientId]);

  // The public demo allows 30 requests a minute per workspace, so each action refetches only what
  // it can have changed: a chat turn never changes consent, and only remember_note changes notes.
  const refreshClient = useCallback(async (id: string, parts: { consents?: boolean; notes?: boolean } = { consents: true }) => {
    const [nextConsents, nextLedger, nextNotes] = await Promise.all([
      parts.consents ? api<Consent[]>(`v1/clients/${id}/consents`) : Promise.resolve(null),
      api<LedgerEntry[]>(`v1/clients/${id}/ledger`),
      parts.notes ? api<AgentNote[]>(`v1/clients/${id}/notes`) : Promise.resolve(null),
    ]);
    if (nextConsents) setConsents(nextConsents);
    setLedger(nextLedger);
    if (nextNotes) setNotes(nextNotes);
  }, []);

  useEffect(() => {
    let active = true;
    api<DemoClient[]>("v1/demo/clients")
      .then(async (nextClients) => {
        if (!active) return;
        setClients(nextClients);
        const first = nextClients[0]?.id ?? "";
        setClientId(first);
        if (first) await refreshClient(first, { consents: true, notes: true });
        if (active) setStatus("Test data ready");
      })
      .catch((error: Error) => active && setStatus(error.message));
    return () => { active = false; };
  }, [refreshClient]);

  async function selectClient(id: string) {
    setClientId(id);
    setTurn(null);
    setStatus("Loading client boundary");
    try {
      await refreshClient(id, { consents: true, notes: true });
      setStatus("Client boundary loaded");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Request failed");
    }
  }

  async function sendMessage(prompt = message) {
    if (!clientId || busy) return;
    setBusy(true);
    setMessage(prompt);
    const localTurn = /\b(ssn|social security)\b/i.test(prompt);
    setStatus(localTurn ? LOCAL_PROGRESS : "Running one bounded assistant turn");
    try {
      const response = await api<ChatResponse>(`v1/clients/${clientId}/chat`, {
        method: "POST",
        body: JSON.stringify({ message: prompt }),
      }, (progress) => {
        setStatus(localTurn ? LOCAL_PROGRESS : progress);
      });
      setTurn(response);
      await refreshClient(clientId, { notes: response.steps.some((step) => step.tool_name === "remember_note") });
      setStatus("Turn complete");
    } catch (error) {
      setTurn(null);
      setStatus(error instanceof Error ? error.message : "Turn failed");
    } finally {
      setBusy(false);
    }
  }

  async function mintConsent(event: FormEvent) {
    event.preventDefault();
    if (!clientId || busy) return;
    setBusy(true);
    setStatus("Minting test consent");
    try {
      await api<Consent>(`v1/clients/${clientId}/consents`, {
        method: "POST",
        body: JSON.stringify({
          kind,
          recipient: kind === "disclose" ? recipient : null,
          purpose: PURPOSE,
          expires_at: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
          pin: TEST_PIN,
        }),
      });
      await refreshClient(clientId);
      setStatus("Test consent active");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Consent failed");
    } finally {
      setBusy(false);
    }
  }

  async function revokeConsent(id: string) {
    setBusy(true);
    setStatus("Revoking test consent");
    try {
      await api<Consent>(`v1/clients/${clientId}/consents/${id}/revoke`, { method: "POST" });
      await refreshClient(clientId);
      setStatus("Test consent revoked");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Revocation failed");
    } finally {
      setBusy(false);
    }
  }

  async function resetDemo() {
    setBusy(true);
    setStatus("Resetting test data");
    try {
      const nextClients = await api<DemoClient[]>("v1/demo/reset", { method: "POST" });
      setClients(nextClients);
      const first = nextClients[0]?.id ?? "";
      setClientId(first);
      setTurn(null);
      setNotes([]);
      if (first) await refreshClient(first);
      setStatus("Test data reset");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Reset failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main>
      <header className="hero">
        <div className="mark" aria-label="Pru"><span>P</span></div>
        <div className="hero-copy">
          <p className="eyebrow">PRIVATE AI, VISIBLE DECISIONS</p>
          <h1>Personal data stays close.<br />Useful work still gets done.</h1>
          <p className="dek">Watch Pru route SSN-bearing context to a local NVIDIA model and consented, SSN-free work to Nemotron on Nebius Token Factory.</p>
        </div>
        <div className="hero-actions">
          <span className="stamp">TEST DATA</span>
          <SquashButton tone="paper" onClick={resetDemo} disabled={busy}>Reset demo</SquashButton>
        </div>
      </header>

      <div className="legal-note">
        <strong>PUBLIC DEMO.</strong> This demo accepts test clients only. An unused browser workspace expires after 10 idle minutes. After you mint consent or run chat, it expires after one idle hour. Do not enter real taxpayer data. Pru refuses it under 26 U.S.C. 7216.
      </div>

      <section className="client-strip" aria-label="Select a test client">
        <label htmlFor="client">Client boundary</label>
        <select id="client" value={clientId} onChange={(event) => void selectClient(event.target.value)} disabled={busy}>
          {clients.map((client) => <option key={client.id} value={client.id}>{client.name} · TEST DATA</option>)}
        </select>
        <span className="client-email">{selected?.email}</span>
        <p className="status" aria-live="polite">{status}</p>
      </section>

      <div className="workspace">
        <SectionCard eyebrow="01 · TRY THE BOUNDARY" title="Ask Pru" className="chat-card">
          <p className="route-note">
            <strong>LOCAL:</strong> {localModelLocation(hosted)}
          </p>
          <div className="presets">
            {PRESETS.map((preset, index) => (
              <button key={preset} type="button" onClick={() => void sendMessage(preset)} disabled={busy}>
                <span>0{index + 1}</span>{preset}
              </button>
            ))}
          </div>
          <label className="field-label" htmlFor="message">One assistant turn</label>
          <textarea id="message" maxLength={4000} value={message} onChange={(event) => setMessage(event.target.value)} />
          <SquashButton onClick={() => void sendMessage()} disabled={busy || !clientId}>{busy ? "Working…" : "Run this turn"}</SquashButton>
          {turn && (
            <article className="answer" aria-label="Assistant result">
              <p className="eyebrow">ASSISTANT ANSWER</p>
              {turn.agent ? <p className="agent-line" data-agent>Answered by {turn.agent.name} {turn.agent.version}</p> : null}
              <p>{turn.answer}</p>
              <p className="eyebrow">WHAT HAPPENED, IN ORDER</p>
              <ol className="step-list" aria-label="Turn trace">
                {turn.steps.map((step) => (
                  <li className="step" key={step.ledger_entry_id} data-step={step.tool_name ?? step.kind}>
                    <div>
                      <RouteBadge route={step.route} />
                      <strong>{decisionLabel(step.decision)}</strong>
                      <span className="step-title">{stepTitle(step)}</span>
                      {step.dry_run ? <span className="dry-run">DRY RUN, NOTHING SENT</span> : null}
                    </div>
                    <p>{step.policy_reason}</p>
                    {step.redacted_spans ? (
                      <p className="redacted">{step.redacted_spans} sensitive {step.redacted_spans === 1 ? "span was" : "spans were"} removed before this call.</p>
                    ) : null}
                    <dl>
                      <div><dt>Model</dt><dd>{step.model_id ?? "none"}</dd></div>
                      <div><dt>Latency</dt><dd>{step.latency_ms} ms</dd></div>
                      <div><dt>Tokens</dt><dd>{step.input_tokens} in / {step.output_tokens} out</dd></div>
                      <div><dt>Cost</dt><dd>${step.cost_usd.toFixed(8)}</dd></div>
                    </dl>
                  </li>
                ))}
              </ol>
            </article>
          )}
        </SectionCard>

        <SectionCard eyebrow="02 · CLIENT PERMISSION" title="Consent ledger" className="consent-card">
          <form onSubmit={(event) => void mintConsent(event)}>
            <label className="field-label" htmlFor="kind">Consent kind</label>
            <select id="kind" value={kind} onChange={(event) => {
              const next = event.target.value as "use" | "disclose";
              setKind(next);
              setRecipient(next === "disclose" ? selected?.email ?? "" : "");
            }}>
              <option value="use">use</option>
              <option value="disclose">disclose</option>
            </select>
            {kind === "disclose" && <><label className="field-label" htmlFor="recipient">Recipient</label><input id="recipient" value={recipient} onChange={(event) => setRecipient(event.target.value)} required /></>}
            <div className="pin-slip"><span>TEST ONLY PIN</span><strong>{TEST_PIN}</strong></div>
            <SquashButton type="submit" disabled={busy || !clientId}>Mint test consent</SquashButton>
          </form>
          <div className="consent-list">
            {consents.length === 0 && <p className="empty">No consent exists for this client.</p>}
            {consents.map((consent) => (
              <article key={consent.id} className={consent.revoked ? "consent revoked" : "consent"}>
                <div><strong>{consent.kind}</strong><span>{consent.revoked ? "REVOKED" : "ACTIVE"}</span></div>
                <p>{consent.recipient ?? "local and approved processor use"}</p>
                {!consent.revoked && <SquashButton tone="wax" onClick={() => void revokeConsent(consent.id)} disabled={busy}>Revoke</SquashButton>}
              </article>
            ))}
          </div>
          <div className="memory" aria-label="What Pru remembers about this client">
            <p className="eyebrow">WHAT PRU REMEMBERS ABOUT THIS CLIENT</p>
            {notes.length === 0 ? (
              <p className="empty">No notes yet. Ask Pru to remember something about this client.</p>
            ) : (
              <ul>
                {notes.map((note) => (
                  <li key={note.id} data-note>
                    <p>{note.text}</p>
                    <span className="hash">Ledger entry {note.ledger_entry_id}</span>
                  </li>
                ))}
              </ul>
            )}
            <p className="small-note">Notes stay in this browser workspace, one client at a time, and Reset demo clears them.</p>
          </div>
        </SectionCard>

        <SectionCard eyebrow="03 · EVERY DECISION" title="Egress ledger" className="ledger-card">
          <p className="small-note">Newest first. The keyed span hash proves the same sensitive span was seen without storing its value.</p>
          <div className="ledger-list">
            {ledger.length === 0 && <p className="empty">Run a prompt to create a decision entry.</p>}
            {ledger.map((entry) => (
              <article className="ledger-row" key={entry.id}>
                <div><RouteBadge route={entry.route} /><strong className={`decision decision-${entry.decision}`}>{decisionLabel(entry.decision)}</strong></div>
                <p>{entry.policy_reason}</p>
                <dl>
                  <div><dt>Action</dt><dd>{entry.action}</dd></div>
                  <div><dt>Model</dt><dd>{entry.model_id ?? "none"}</dd></div>
                  <div><dt>Entry</dt><dd className="hash">{entry.id}</dd></div>
                  <div><dt>Span hash</dt><dd className="hash">{entry.span_hashes[0] ?? "none"}</dd></div>
                </dl>
              </article>
            ))}
          </div>
        </SectionCard>
      </div>

      <SectionCard eyebrow="04 · EVIDENCE, NOT A PROMISE" title="What is proved" className="proof-card">
        <div className="proof-grid">
          <a href="https://github.com/StephenSook/pru/actions/workflows/ci.yml" target="_blank" rel="noreferrer"><strong>SymCC + cvc5</strong><span>Finds policy divergence and proves the concrete witness.</span></a>
          <a href="https://github.com/StephenSook/pru/blob/main/eval/boundaries/synthetic-client.yaml" target="_blank" rel="noreferrer"><strong>OpenShell boundary</strong><span>Host and sandbox probes produce the committed boundary receipt.</span></a>
          <a href="https://github.com/StephenSook/pru/blob/main/eval/ssn_canaries.json" target="_blank" rel="noreferrer"><strong>SSN canaries</strong><span>360 of 360 positives detected. 0 of 120 negative controls flagged.</span></a>
        </div>
        <p className="not-proved"><strong>What is not proved:</strong> It does not prove that the gateway&apos;s context labels are truthful.</p>
      </SectionCard>

      <footer><span>Pru</span><p>One client boundary at a time.</p></footer>
    </main>
  );
}
