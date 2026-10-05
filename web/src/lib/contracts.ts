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

export type ChatStep = {
  kind: string;
  route: string;
  model_id: string | null;
  latency_ms: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  decision: string;
  policy_reason: string;
  ledger_entry_id: string;
  dry_run: boolean;
  tool_name: string | null;
};

export type ChatResponse = {
  turn_id: string;
  answer: string;
  steps: ChatStep[];
};
