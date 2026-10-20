"use client";

import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentNote, ChatResponse, Consent, DemoClient, LedgerEntry } from "@/lib/contracts";
import { ENDPOINT_TEST, seconds } from "@/lib/measured";
import { decisionLabel, reasonText } from "@/lib/turn";
import { RouteTradeoff, RunProgress } from "@/ui/Progress";
import { LiveTurn, RecordedTurn, RouteBadge } from "@/ui/Turn";

const PURPOSE = "prepare this synthetic tax client's return";

// The two consents Pru records, in a preparer's words. A disclosure always names its recipient,
// which can be the client's own address, so it is not called a third-party disclosure here.
const CONSENT_KINDS: Record<Consent["kind"], string> = {
  use: "Use for preparing this return",
  disclose: "Disclose to a named recipient",
};

function consentLabel(consent: Consent): string {
  return consent.kind === "disclose" ? `Disclose to ${consent.recipient}` : CONSENT_KINDS.use;
}
const TEST_PIN = "53179";

// Each preset says what the gateway will do with it, in the terms of its own rules: a draft needs a
// disclose consent naming the recipient and is never sent, an SSN may only go to the local model,
// and tool results have the SSN removed before the agent reads them.
export const PRESETS = [
  {
    prompt: "Draft an email to the client asking for their 1099-INT",
    outcome: "Needs a disclose consent naming the client's email. Then drafted, never sent.",
  },
  {
    prompt: "Check this client's SSN against the W-2 with the local model.",
    outcome: `Runs on the local model, measured at ${seconds(ENDPOINT_TEST.local.repeatTurnSeconds)} to ${seconds(ENDPOINT_TEST.local.firstTurnSeconds)}. The SSN never goes to Token Factory.`,
  },
  {
    prompt: "Email this client's W-2 details to marketing@example.com",
    outcome: "Blocked unless a disclose consent names marketing@example.com.",
  },
  {
    prompt: "Read the client's W-2 and report box 1 wages",
    outcome: "The SSN is removed from the W-2 before Token Factory reads it.",
  },
  {
    prompt: "Remember that this client prefers callback appointments on Tuesday mornings",
    outcome: "Saved as a note in this client's file, with a ledger entry.",
  },
  {
    prompt: "Explain the current consent state for this client",
    outcome: "Reads the consents on file and explains them.",
  },
] as const;

// The gateway sends SSN-bearing work to the local model, so a request that names the SSN is the
// slow one worth warning about while it runs.
function namesTheSsn(prompt: string): boolean {
  return /\b(ssn|social security)\b/i.test(prompt);
}

// Read only from event handlers, when a turn starts.
function wallClock(): number {
  return Date.now();
}

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

// One part of the form: a reversed "Part" tab and its title, the way a return divides its pages.
function Part({ numeral, title, id, className = "", children }: Readonly<{
  numeral: string;
  title: string;
  id: string;
  className?: string;
  children: React.ReactNode;
}>) {
  return (
    <section className={`part ${className}`} aria-labelledby={id}>
      <h2 id={id} className="part-head"><span className="part-tab">Part {numeral}</span>{title}</h2>
      {children}
    </section>
  );
}

function SquashButton({ children, tone = "plum", ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: "plum" | "wax" | "paper";
}) {
  return <button className={`squash squash-${tone}`} {...props}>{children}</button>;
}

export function localModelLocation(hosted: boolean): string {
  return hosted
    ? "this server's own model, never sent to a model provider"
    : "this PC's own model, never sent to a model provider";
}

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

export function JudgeDoor({ hosted }: Readonly<{ hosted: boolean }>) {
  const [clients, setClients] = useState<DemoClient[]>([]);
  const [clientId, setClientId] = useState("");
  const [consents, setConsents] = useState<Consent[]>([]);
  const [ledger, setLedger] = useState<LedgerEntry[]>([]);
  const [notes, setNotes] = useState<AgentNote[]>([]);
  const [turn, setTurn] = useState<ChatResponse | null>(null);
  const [message, setMessage] = useState<string>(PRESETS[0].prompt);
  const [kind, setKind] = useState<"use" | "disclose">("use");
  const [recipient, setRecipient] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("Loading test clients");
  const [run, setRun] = useState<{ prompt: string; isLocal: boolean; startedAt: number } | null>(null);
  const [progressMessage, setProgressMessage] = useState("");
  const resultRef = useRef<HTMLElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);

  const selected = useMemo(() => clients.find((client) => client.id === clientId), [clients, clientId]);
  const activeConsents = consents.filter((consent) => !consent.revoked);

  // When a turn finishes, bring its result into view and move focus to it, so a phone user who
  // tapped a preset above the fold sees the answer without hunting for it.
  useEffect(() => {
    if (!turn) return;
    const result = resultRef.current;
    if (!result) return;
    result.focus({ preventScroll: true });
    result.scrollIntoView({ block: "nearest", behavior: prefersReducedMotion() ? "auto" : "smooth" });
  }, [turn]);

  // When a turn starts, make its progress panel visible too, without moving focus.
  useEffect(() => {
    if (!run) return;
    progressRef.current?.scrollIntoView({ block: "nearest", behavior: prefersReducedMotion() ? "auto" : "smooth" });
  }, [run]);

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
    setStatus("Loading client file");
    try {
      await refreshClient(id, { consents: true, notes: true });
      setStatus("Client file loaded");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Request failed");
    }
  }

  async function sendMessage(prompt = message) {
    if (!clientId || busy) return;
    setBusy(true);
    setMessage(prompt);
    setRun({ prompt, isLocal: namesTheSsn(prompt), startedAt: wallClock() });
    setProgressMessage("");
    setStatus("Running one request");
    try {
      const response = await api<ChatResponse>(`v1/clients/${clientId}/chat`, {
        method: "POST",
        body: JSON.stringify({ message: prompt }),
      }, (progress) => {
        setProgressMessage(progress);
        setStatus(progress);
      });
      setTurn(response);
      await refreshClient(clientId, { notes: response.steps.some((step) => step.tool_name === "remember_note") });
      setStatus("Request finished");
    } catch (error) {
      setTurn(null);
      setStatus(error instanceof Error ? error.message : "Request failed");
    } finally {
      setRun(null);
      setBusy(false);
    }
  }

  async function recordConsent(nextKind: "use" | "disclose", nextRecipient: string | null) {
    if (!clientId || busy) return;
    setBusy(true);
    setStatus("Recording client consent");
    try {
      await api<Consent>(`v1/clients/${clientId}/consents`, {
        method: "POST",
        body: JSON.stringify({
          kind: nextKind,
          recipient: nextKind === "disclose" ? nextRecipient : null,
          purpose: PURPOSE,
          expires_at: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
          pin: TEST_PIN,
        }),
      });
      await refreshClient(clientId);
      setStatus("Client consent recorded");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Recording consent failed");
    } finally {
      setBusy(false);
    }
  }

  function mintConsent(event: FormEvent) {
    event.preventDefault();
    void recordConsent(kind, recipient);
  }

  async function revokeConsent(id: string) {
    setBusy(true);
    setStatus("Revoking client consent");
    try {
      await api<Consent>(`v1/clients/${clientId}/consents/${id}/revoke`, { method: "POST" });
      await refreshClient(clientId);
      setStatus("Client consent revoked");
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
      <header className="form-head">
        <p className="form-number"><span className="form-word">Form</span><span className="form-name">Pru</span></p>
        <div className="form-title">
          <h1>Pru checks every AI request against the client&apos;s signed consent.</h1>
          <p className="dek">Work that names the SSN stays on a local NVIDIA Nemotron 3 Nano 4B. Consented work with the SSN removed goes to Nemotron 3.5 Lightning on Nebius Token Factory. Every decision is written to the client&apos;s ledger.</p>
        </div>
        <div className="form-corner">
          <p className="test-only">Test data only</p>
          <SquashButton tone="paper" onClick={resetDemo} disabled={busy}>Reset demo</SquashButton>
        </div>
      </header>

      <p className="legal-note">
        <strong>Public demo.</strong> This demo accepts test clients only. An unused browser workspace expires after 10 idle minutes. After you record consent or run a request, it expires after one idle hour. Do not enter real taxpayer data. Pru refuses it under 26 U.S.C. 7216.
      </p>

      <section className="client-strip" aria-label="Select a test client">
        <div className="box box-client">
          <label className="box-label" htmlFor="client">Client file</label>
          <select id="client" value={clientId} onChange={(event) => void selectClient(event.target.value)} disabled={busy}>
            {clients.map((client) => <option key={client.id} value={client.id}>{client.name} (test data)</option>)}
          </select>
        </div>
        <div className="box">
          <span className="box-label">Email on file</span>
          <span className="typed client-email">{selected?.email}</span>
        </div>
        <div className="box box-consent" data-consent-on-file>
          <span className="box-label">Consent on file</span>
          {activeConsents.length === 0 ? (
            <span className="consent-none">
              <span>None. Pru refuses every request until one is recorded.</span>
              <button type="button" className="inline-action" onClick={() => void recordConsent("use", null)} disabled={busy || !clientId}>
                Record a use consent
              </button>
            </span>
          ) : (
            <span className="typed">
              {activeConsents.map((consent) => consentLabel(consent)).join("; ")}
            </span>
          )}
        </div>
        <div className="box box-status">
          <span className="box-label">Status</span>
          <p className="status" aria-live="polite">{status}</p>
        </div>
      </section>

      <Part numeral="I" title="Ask Pru about this client" id="part-ask" className="part-ask">
        <div className="ask-grid">
          <div className="ask-controls">
            <p className="route-note">
              <strong>Local model:</strong> {localModelLocation(hosted)}
            </p>
            <div className="presets">
              {PRESETS.map((preset, index) => (
                <button
                  key={preset.prompt}
                  type="button"
                  className="preset"
                  onClick={() => void sendMessage(preset.prompt)}
                  disabled={busy}
                  aria-labelledby={`preset-${index}`}
                  aria-describedby={`preset-${index}-outcome`}
                >
                  <span className="preset-prompt" id={`preset-${index}`}>{preset.prompt}</span>
                  <span className="preset-outcome" id={`preset-${index}-outcome`}>{preset.outcome}</span>
                </button>
              ))}
            </div>
            <RouteTradeoff hosted={hosted} />
            <label className="field-label" htmlFor="message">Or write your own request</label>
            <textarea id="message" maxLength={4000} value={message} onChange={(event) => setMessage(event.target.value)} />
            <SquashButton onClick={() => void sendMessage()} disabled={busy || !clientId}>{busy ? "Working…" : "Send to Pru"}</SquashButton>
          </div>
          <div className="ask-result" aria-busy={run ? true : undefined}>
            {run ? (
              <RunProgress
                key={run.startedAt}
                prompt={run.prompt}
                isLocal={run.isLocal}
                hosted={hosted}
                startedAt={run.startedAt}
                message={progressMessage}
                panelRef={progressRef}
              />
            ) : null}
            {turn ? <LiveTurn turn={turn} resultRef={resultRef} /> : <RecordedTurn />}
          </div>
        </div>
      </Part>

      <div className="lower-parts">
        <Part numeral="II" title="Client consent (Section 7216)" id="part-consent" className="consent-card">
          <form onSubmit={mintConsent}>
            <label className="field-label" htmlFor="kind">Type of consent</label>
            <select id="kind" value={kind} onChange={(event) => {
              const next = event.target.value as "use" | "disclose";
              setKind(next);
              setRecipient(next === "disclose" ? selected?.email ?? "" : "");
            }}>
              <option value="use">{CONSENT_KINDS.use}</option>
              <option value="disclose">{CONSENT_KINDS.disclose}</option>
            </select>
            {kind === "disclose" && <><label className="field-label" htmlFor="recipient">Recipient&apos;s email</label><input id="recipient" value={recipient} onChange={(event) => setRecipient(event.target.value)} required /></>}
            <div className="pin-slip"><span>Client signing PIN (test data only)</span><strong>{TEST_PIN}</strong></div>
            <SquashButton type="submit" disabled={busy || !clientId}>Record client consent (Section 7216)</SquashButton>
          </form>
          <div className="consent-list">
            {consents.length === 0 && <p className="empty">No consent exists for this client.</p>}
            {consents.map((consent) => (
              <article key={consent.id} className={consent.revoked ? "consent revoked" : "consent"}>
                <div><strong>{CONSENT_KINDS[consent.kind]}</strong><span>{consent.revoked ? "Revoked" : "Active"}</span></div>
                <p>{consent.recipient ? `Recipient: ${consent.recipient}` : "Covers the local model and Token Factory as an approved processor."}</p>
                {!consent.revoked && <SquashButton tone="wax" onClick={() => void revokeConsent(consent.id)} disabled={busy}>Revoke</SquashButton>}
              </article>
            ))}
          </div>
          <div className="memory" aria-label="What Pru remembers about this client">
            <h3 className="sub-head">What Pru remembers about this client</h3>
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
        </Part>

        <Part numeral="III" title="Where this client's data went" id="part-ledger" className="ledger-card">
          <p className="small-note">Newest first, one row per gateway decision. The SSN fingerprint is a keyed hash, so the same SSN is recognized again without being stored.</p>
          <div className="ledger-list">
            {ledger.length === 0 && <p className="empty">Run a request to see where this client&apos;s data went.</p>}
            {ledger.map((entry) => (
              <article className="ledger-row" key={entry.id}>
                <div><RouteBadge route={entry.route} /><strong className={`decision decision-${entry.decision}`}>{decisionLabel(entry.decision)}</strong></div>
                <p>{reasonText(entry.policy_reason)} <span className="reason-code">({entry.policy_reason})</span></p>
                <dl>
                  <div><dt>Action</dt><dd>{entry.action}</dd></div>
                  <div><dt>Model</dt><dd>{entry.model_id ?? "none"}</dd></div>
                  <div><dt>Ledger entry</dt><dd className="hash">{entry.id}</dd></div>
                  <div><dt>Fingerprint of the SSN seen (value not stored)</dt><dd className="hash">{entry.span_hashes[0] ?? "none"}</dd></div>
                </dl>
              </article>
            ))}
          </div>
        </Part>
      </div>

      <Part numeral="IV" title="What is proved" id="part-proof" className="proof-card">
        <div className="proof-grid">
          <a href="https://github.com/StephenSook/pru/actions/workflows/ci.yml" target="_blank" rel="noreferrer"><strong>SymCC + cvc5</strong><span>Finds policy divergence and proves the concrete witness.</span></a>
          <a href="https://github.com/StephenSook/pru/blob/main/eval/boundaries/synthetic-client.yaml" target="_blank" rel="noreferrer"><strong>OpenShell boundary</strong><span>Host and sandbox probes produce the committed boundary receipt.</span></a>
          <a href="https://github.com/StephenSook/pru/blob/main/eval/ssn_canaries.json" target="_blank" rel="noreferrer"><strong>SSN canaries</strong><span>360 of 360 positives detected. 0 of 120 negative controls flagged.</span></a>
        </div>
        <p className="not-proved"><strong>What is not proved:</strong> It does not prove that the gateway&apos;s context labels are truthful.</p>
      </Part>

      <footer><span>Pru</span><p>One client file at a time.</p></footer>
    </main>
  );
}
