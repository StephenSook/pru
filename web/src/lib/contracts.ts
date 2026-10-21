export type DemoClient = {
  id: string;
  name: string;
  email: string;
  test_data: boolean;
};

export type Consent = {
  id: string;
  kind: "use" | "disclose";
  recipient: string | null;
  purpose: string;
  signed_at: string;
  expires_at: string;
  revoked: boolean;
};

export type LedgerEntry = {
  id: string;
  at: string;
  client_id: string;
  action: string;
  decision: string;
  route: string;
  policy_reason: string;
  model_id?: string;
  contains_ssn: boolean;
  span_hashes: string[];
};

// The gateway sends null for a measured field it did not record, never a stand-in zero.
export type ChatStep = {
  kind: string;
  route: string;
  model_id: string | null;
  latency_ms: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cost_usd: number | null;
  decision: string;
  policy_reason: string;
  ledger_entry_id: string;
  dry_run: boolean;
  tool_name: string | null;
  redacted_spans?: number;
};

export type AgentIdentity = {
  name: string;
  version: string;
};

export type ChatResponse = {
  turn_id: string;
  agent?: AgentIdentity;
  answer: string;
  steps: ChatStep[];
};

export type AgentNote = {
  id: string;
  text: string;
  created_at: string;
  ledger_entry_id: string;
};
