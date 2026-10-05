"use client";

import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import type { ChatResponse, Consent, DemoClient, LedgerEntry } from "@/lib/contracts";

const PURPOSE = "prepare this synthetic tax client's return";
const TEST_PIN = "53179";
const PRESETS = [
  "Draft an email to the client asking for their 1099-INT",
  "What is this client's SSN and filing status?",
  "Email this client's W-2 details to marketing@example.com",
] as const;

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api/pru/${path}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : undefined,
  });
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

function RouteBadge({ route }: Readonly<{ route: string }>) {
  const local = route === "local";
  return <span className={`route-badge ${local ? "route-local" : "route-cloud"}`}>{local ? "● LOCAL" : "↗ TOKEN FACTORY"}</span>;
}

export function JudgeDoor() {
  const [clients, setClients] = useState<DemoClient[]>([]);
  const [clientId, setClientId] = useState("");
  const [consents, setConsents] = useState<Consent[]>([]);
  const [ledger, setLedger] = useState<LedgerEntry[]>([]);
  const [turn, setTurn] = useState<ChatResponse | null>(null);
  const [message, setMessage] = useState<string>(PRESETS[0]);
  const [kind, setKind] = useState<"use" | "disclose">("use");
  const [recipient, setRecipient] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("Loading test clients");

  const selected = useMemo(() => clients.find((client) => client.id === clientId), [clients, clientId]);

  const refreshClient = useCallback(async (id: string) => {
    const [nextConsents, nextLedger] = await Promise.all([
      api<Consent[]>(`v1/clients/${id}/consents`),
      api<LedgerEntry[]>(`v1/clients/${id}/ledger`),
    ]);
    setConsents(nextConsents);
    setLedger(nextLedger);
  }, []);

  useEffect(() => {
    let active = true;
    api<DemoClient[]>("v1/demo/clients")
      .then(async (nextClients) => {
        if (!active) return;
        setClients(nextClients);
        const first = nextClients[0]?.id ?? "";
        setClientId(first);
        if (first) await refreshClient(first);
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
      await refreshClient(id);
      setStatus("Client boundary loaded");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Request failed");
    }
  }

  async function sendMessage(prompt = message) {
    if (!clientId || busy) return;
    setBusy(true);
    setMessage(prompt);
    setStatus("Running one bounded assistant turn");
    try {
      const response = await api<ChatResponse>(`v1/clients/${clientId}/chat`, {
        method: "POST",
        body: JSON.stringify({ message: prompt }),
      });
      setTurn(response);
      await refreshClient(clientId);
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

      <div className="legal-note"><strong>TEST DATA ONLY.</strong> Real taxpayer data is barred by 26 U.S.C. 7216.</div>

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
              <p>{turn.answer}</p>
              <div className="step-list">
                {turn.steps.map((step) => (
                  <div className="step" key={step.ledger_entry_id}>
                    <div><RouteBadge route={step.route} /><strong>{step.decision.toUpperCase()}</strong></div>
                    <p>{step.policy_reason}</p>
                    <dl>
                      <div><dt>Model</dt><dd>{step.model_id ?? "none"}</dd></div>
                      <div><dt>Latency</dt><dd>{step.latency_ms} ms</dd></div>
                      <div><dt>Tokens</dt><dd>{step.input_tokens} in / {step.output_tokens} out</dd></div>
                      <div><dt>Cost</dt><dd>${step.cost_usd.toFixed(8)}</dd></div>
                    </dl>
                  </div>
                ))}
              </div>
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
        </SectionCard>

        <SectionCard eyebrow="03 · EVERY DECISION" title="Egress ledger" className="ledger-card">
          <p className="small-note">Newest first. The keyed span hash proves the same sensitive span was seen without storing its value.</p>
          <div className="ledger-list">
            {ledger.length === 0 && <p className="empty">Run a prompt to create a decision entry.</p>}
            {ledger.map((entry) => (
              <article className="ledger-row" key={entry.id}>
                <div><RouteBadge route={entry.route} /><strong className={`decision decision-${entry.decision}`}>{entry.decision.toUpperCase()}</strong></div>
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
