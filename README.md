# Pru

Pru is a tax-practice assistant for a solo preparer or small practice. In the keyed container build, Hermes Agent v0.22.2 runs as one long-lived process under uid 10000 and a Landlock jail. A Rust gateway is Hermes's only model and tool boundary. The gateway checks each action against a signed consent token and Cedar policy before it can leave the client compartment. NVIDIA OpenShell and NemoClaw remain an optional PC-build path from a separate spike; they are not part of the shipped container.

Pru keeps SSN-bearing prompts on the same machine as its gateway and routes them to a local OpenAI-compatible llama-server running NVIDIA Nemotron 3 Nano 4B. It may send consented, SSN-free prompts to NVIDIA Nemotron on Nebius Token Factory. Phase A uses only code-generated synthetic canaries. The product plan permits the IRS's public test taxpayers, labelled as test data, but this repository contains no real taxpayer data.

## What the repository proves

- `pru-ssn` recognizes the required SSN surface forms, including the IRS test group `00`, and labels visible group `00` or area `9xx` identifiers as `test_only`.
- The deterministic canary harness measured 360 detections from 360 generated positive cases and 0 flagged controls from 120 generated negative controls.
- `pru-consent` mints one Ed25519 Biscuit token per consent document. It checks an Argon2id PIN hash, signature, expiry, kind, recipient, purpose, and revocation status.
- `pru-policy` makes the egress decision with Cedar. SymCC and cvc5 prove that the live policy implies reference policy R for both `use` and `disclose` actions.
- The planted Cedar permit does not imply R. SymCC returns a counterexample where the bad policy permits disclosure and R denies it.
- `pru-boundary` emits a REST-only OpenShell policy whose only network host is the Pru gateway. OpenShell prover 0.1.2 reports the compiled policy within its boundary and reports a planted `example.com` rule outside it.
- `pru-gateway` scans every string in each action argument, verifies consent, asks Cedar, writes a JSONL decision ledger, and selects the local or Token Factory route. Its ledger stores keyed span hashes, never raw SSNs.
- The container runs Hermes Agent v0.22.2 with only the gateway's OpenAI-compatible provider and seven MCP tools. Model catalog fetches, tool search, and Hermes's built-in memory are disabled.
- The container smoke test measured Landlock ABI 7 on Docker Desktop, a `0700` visitor root, `0600` client files, and an access denial when uid 10000 tried to read a visitor file.
- Email and calendar endpoints build a dry-run request and ledger entry. They do not send anything in phase A.

## What the repository does not prove

The formal proof covers the Cedar decision logic. It does not prove that the gateway's context labels are truthful. The SSN result is measured recall on this generated canary distribution, not a guarantee over every document or OCR failure. The OpenShell proof covers the prover's REST fragment, not the product container. On a one-hour Nebius endpoint test on 2026-10-20, `/healthz` reported Landlock enforced (ABI 5) with the seccomp socket filter. The repaired live grounding eval measured Lightning at exactly the required 27 of 30 expected first tools. That result does not prove perfect grounding on other prompts. No real practice workflow, Gmail or Calendar send, or real taxpayer data is included.

This is engineering evidence, not a legal opinion about 26 U.S.C. 7216, 26 C.F.R. 301.7216-3, Rev. Proc. 2013-14, or any specific disclosure.

## Test build

The test build gives a judge one URL at `http://127.0.0.1:3000`. The browser talks only to a fixed Next.js server route. The server route talks to the Rust gateway on the container loopback interface. The browser never receives `NEBIUS_API_KEY`.

With a Token Factory key, the image starts four long-lived processes: Next.js, the Axum gateway, a CPU-only llama-server, and Hermes Agent. The gateway owns consent checks, Cedar decisions, model routing, per-client notes, and the JSONL ledger. SSN-bearing work reaches NVIDIA Nemotron 3 Nano 4B only through the local-model tool. SSN-free Hermes model calls default to the gate-selected `nvidia/Nemotron-3_5-Lightning` on Nebius Token Factory. Email and calendar tools remain dry runs.

The container defaults to `PRU_AGENT=hermes`. Without `NEBIUS_API_KEY`, its entrypoint disables Hermes and keeps the existing built-in Rust loop available. With a key, Hermes runs as uid 10000. A per-boot bearer protects `/agent/v1` and `/mcp`. The launcher's Landlock rule lets Hermes open TCP connections only to port 8787, and a seccomp filter allows only Unix sockets and IPv4 or IPv6 TCP streams, so Hermes cannot resolve names, send datagrams, or open raw, packet, netlink, SCTP or MPTCP sockets; io_uring is refused too. Landlock rules name a port, not a host, so a TCP connection to port 8787 on another address is not blocked by the jail itself. Hermes is configured with only the Pru MCP toolset, with no shell or web tools. `PRU_AGENT=builtin` selects the prior Rust loop explicitly.

The two startup clients are TEST DATA. Their generated SSN-shaped values use group `00` or area `9xx`, which the Social Security Administration does not issue. Resetting the demo clears only the current browser's consents, notes, and ledgers, then recreates its clients under `/tmp/pru-data/<workspace-id>/`. Real taxpayer data is barred by 26 U.S.C. 7216.

From PowerShell, build the image and start it without a key:

```powershell
docker build -t pru:test .
docker run --rm --name pru-test -p 3000:3000 pru:test
```

Open `http://127.0.0.1:3000`. The container log says `Token Factory disabled`. Test clients, consent, the local SSN route, and the decision ledger work without a key. A turn that needs Token Factory is refused before network contact with `Token Factory is disabled because this build has no NEBIUS_API_KEY.` The ledger records a `deny` with reason `token_factory_not_configured`; the page shows the same refusal in plain words.

To exercise Token Factory, stop the keyless container and start a new one with the key inherited from the PowerShell environment. The key is not copied into an image layer.

```powershell
docker stop pru-test
$env:NEBIUS_API_KEY = [Environment]::GetEnvironmentVariable('NEBIUS_API_KEY', 'User')
docker run --rm --name pru-test -p 3000:3000 -e NEBIUS_API_KEY pru:test
```

Stop either container with `docker stop pru-test`.

## Agent isolation by build

| Build | Where Hermes runs | Network path for Hermes | Enforced by | Evidence |
|---|---|---|---|---|
| Test build with a Token Factory key | In the Pru container as uid 10000 | TCP to port 8787 only (the gateway is `127.0.0.1:8787`); no other socket except Unix sockets | Landlock TCP-connect rule, a seccomp socket filter, Unix permissions, and a per-boot gateway credential | Container smoke measured Landlock ABI 7 and denied uid 10000 access to a `0600` visitor file |
| Hosted, December 1 to 15 | Same image | Same | Same if the endpoint kernel has Landlock ABI 4 or newer; otherwise the built-in loop runs | 2026-10-20 and 2026-10-21 endpoint tests: `/healthz` reported `Hermes Agent v0.22.2` and `Landlock enforced (ABI 5) + seccomp socket filter` ([2026-10-20](eval/endpoint_test-2026-10-20/summary.json), [2026-10-21](eval/endpoint_test-2026-10-21/summary.json)) |
| Optional PC build | OpenShell sandbox through NemoClaw | OpenShell REST policy to the Pru gateway | OpenShell 0.0.116 in the prior spike | Hermes spike measured; Pru wiring is NOT VERIFIED and is not shipped here |

The visitor data root is mode `0700`, files inherit mode `0600` from `umask 077`, and Hermes receives neither `NEBIUS_API_KEY` nor permission to read that root. Landlock restricts TCP connect by port; the seccomp filter refuses every other socket family and protocol, including through x32 syscall numbers, and `scripts/jail-socket-check.py` checks that inside the container. On the Nebius endpoint, `/healthz` reported Landlock ABI 5 with the seccomp socket filter on 2026-10-20 and again on 2026-10-21.

## Hermes Agent wiring

Hermes uses `POST /agent/v1/chat/completions` as its model provider and `POST /mcp` as its only tool server. The gateway strips upstream `reasoning_effort` and `stream`, overwrites the model id, applies consent and Cedar, scans for SSNs, reserves hosted tokens, and writes the trace ledger. The seven MCP tools are `get_client_summary`, `get_consent_state`, `read_document`, `ask_local_model`, `draft_client_email`, `propose_calendar_slot`, and `remember_note`.

Only one Hermes turn may run at a time. Each browser turn gets a fresh Hermes session id, and the gateway deletes that session after the response. Cleanup retries a temporary Hermes `session_active_turn` conflict five times with at most one second of total delay. Model catalog fetches, tool search, title generation, background review, and Hermes's built-in memory are disabled.

Per-client notes are plain JSON lines in the client's workspace directory. Only `remember_note` can write them, each note is limited to 280 characters, each client is limited to 20 notes, and reset clears them. The gateway inserts notes into later turn preambles. They are deleted with the workspace after one idle hour on the hosted door and are not encrypted at rest.

## Hosted during judging

The hosted copy is at <https://port3000-najy7whtsgwrd47.tunnel.applications.us-north1.nebius.cloud>. It runs the same image on a Nebius Serverless endpoint (`cpu-d3`, 4 vCPU, 16 GB RAM) from December 1 through December 15, 2026, the judging window. Outside that window the endpoint is stopped and the URL answers `404`, so use the test build above.

The endpoint was created on 2026-10-21 from the image built at `d9e9122`. A browser script then ran four turns on it through the page's own controls: a local SSN check in 13.0 s, a refused marketing email in 1.9 s, and two Token Factory turns in 1.4 s each, with no console errors. The endpoint was then stopped and started again. The URL stayed the same, `/healthz` reported ready after the restart, and the restart took about 9 minutes because the image is pulled again ([receipt](eval/endpoint_test-2026-10-21/summary.json)). An earlier one-hour test endpoint ran on 2026-10-20 and was deleted; its saved responses are in [`eval/endpoint_test-2026-10-20/`](eval/endpoint_test-2026-10-20).

The hosted image sits behind the endpoint's managed HTTPS ingress. The public page accepts test clients only and must never receive real taxpayer data. Set `PRU_HOSTED_DOOR=1` so the page describes `LOCAL` as `this server's own model, never sent to a model provider`. Keep `PRU_LOCAL_THREADS=4` explicit in the deployment even though the container now defaults to `nproc`.

Each visitor receives a random UUIDv4 in the `pru_workspace` cookie. The cookie is `HttpOnly`, `SameSite=Lax`, scoped to `/`, and renewed with a 3,600-second lifetime on each API response. The Next.js server validates the cookie and sends the workspace id to the loopback-only gateway. Browser credentials and caller-supplied internal headers are not forwarded.

The gateway stores that visitor's two TEST DATA clients, consent records, notes, and ledgers under `/tmp/pru-data/<workspace-id>/`. `Reset demo` takes an exclusive lock and resets only that directory. A workspace that has never minted consent or started chat expires after 600 idle seconds. Once either action touches it, the idle lifetime is 3,600 seconds. Both rules survive process restarts. Cleanup runs every 60 seconds. At most 200 workspaces may be active at once. At capacity, the gateway evicts the least recently used untouched workspace. It never evicts a touched workspace early; if all 200 are touched, a new visitor receives a clear `503` response.

The public limits are fixed in code and covered by tests:

| Control | Value |
|---|---:|
| Request body | 16,384 bytes |
| Request window | 60 seconds |
| Requests per workspace per window | 30 |
| Requests per IP per window | 120 |
| Active workspaces | 200 |
| Untouched workspace idle lifetime | 600 seconds |
| Touched workspace idle lifetime | 3,600 seconds |
| Workspace cleanup interval | 60 seconds |
| Token Factory budget per UTC day | 1,000,000 tokens |
| Token Factory reservation per in-flight call | 32,000 tokens |
| Concurrent local-model generations | 1 |

The hosted-token budget belongs to the whole instance and is persisted at `/tmp/pru-data/.instance/token-budget.json`. Each hosted call reserves 32,000 tokens before network contact, then reconciles the reservation to the provider's `usage.total_tokens`. A failed or malformed hosted response keeps the full reservation charged. Once the 1,000,000-token daily budget is spent, hosted calls receive a plain `429` message until the next UTC day. Local-model requests never consume that budget. A second concurrent local generation waits up to 90 seconds for the one local slot while the page shows progress, and receives a plain `429` busy message only if the slot stays taken.

Set `PRU_TRUSTED_CLIENT_IP_HEADER` only after checking what the hosting ingress overwrites or appends. For a single overwritten value, set only that variable. For an appended `X-Forwarded-For` chain, also set `PRU_TRUSTED_PROXY_HOPS` to the ingress position counted from the right. The proxy validates the selected value as an IPv4 or IPv6 address. An invalid header or hop count uses the fail-closed `unknown` IP bucket. Probed on 2026-10-20 with a header-echo endpoint behind the same Nebius endpoint ingress (us-north1): the ingress replaced a supplied `X-Forwarded-For` with the caller's single public address and passed supplied `X-Real-Ip` and `True-Client-Ip` values through unchanged. So set `PRU_TRUSTED_CLIENT_IP_HEADER=x-forwarded-for` with no hop count, and never trust `X-Real-Ip` or `True-Client-Ip` there.

Visitor input containing an SSN-shaped value is accepted only when the value is in a never-issued range: area `000`, area `666`, area `900` through `999`, group `00`, or serial `0000`. Any other SSN-shaped value is refused before workspace creation, model contact, ledger writes, or value-bearing logs. The response never repeats the value and says: `This public demo accepts test data only. Do not enter real taxpayer data. 26 U.S.C. 7216.`

The host polls `GET /healthz`. It returns `200` only when the Next.js route can reach the gateway and the gateway can reach llama-server's local `/health` endpoint. It never calls Token Factory. During startup or after a local-model failure it returns `503` with component states. The image gives the CPU model a 210-second health-check start period.

Chat responses stream an immediate progress record and a heartbeat every 10 seconds until the gateway returns. For a local turn, the page says `Local Nemotron Nano 4B is reading this on the server CPU, first answer can take about 30 s`. On this PC with 4 CPUs and `PRU_LOCAL_THREADS=4`, the measured first local turn took 24.65 to 27.16 seconds across three fresh containers. Repeated or closely related turns took 4.62 to 6.73 seconds. Peak container memory during a cold local turn was 4,284,825,600 bytes. On the 2026-10-20 Nebius test endpoint (cpu-d3, 4 vCPU), a first local turn took 11.1 s and a repeat 5.0 s, and a Token Factory turn took 2.9 s (curl wall times, [receipt](eval/endpoint_test-2026-10-20/summary.json)). Memory use, cold-start time and the managed ingress idle cutoff there are NOT VERIFIED.

The local model layer is pinned and checked during the build:

| Field | Value |
|---|---|
| Hugging Face repository | [`nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF`](https://huggingface.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF) |
| Repository revision | `ba223d14e45525f7fae81db77ea8cabeb2fc6c25` |
| File | `NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf` |
| File size | 2,837,072,864 bytes |
| SHA-256 | `be5d9a656a51922f24f1f09a759cebb694e1f5d9728bf0ef9f8c972c5a0b5ef2` |
| License | [NVIDIA Nemotron Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-nemotron-open-model-license/) |

llama.cpp is pinned to commit `a7fb71fab83b474a0892b9a05aaa3a8ddca2729b`, release `b11401`. The hosting scout ran the repository benchmark inside Docker Desktop on an Intel i7-9700K with 4 CPUs, `PRU_LOCAL_THREADS=4`, a 300-token prompt, a forced 150-token answer, temperature 0, and three runs.

| Run | First token | Output speed |
|---:|---:|---:|
| 1 | 11.5444 s | 5.9724 tokens/s |
| 2 | 12.0676 s | 5.6617 tokens/s |
| 3 | 10.9790 s | 4.7124 tokens/s |
| Median | 11.5444 s | 5.6617 tokens/s |

Re-run the measurement inside a running container:

```powershell
docker exec pru-test node /app/benchmark-local-model.mjs
```

This build wires Hermes directly under Landlock, not through OpenShell or NemoClaw. It does not include IRS ATS test taxpayers, Gmail or Calendar sending, a Windows Hello signer, or per-client SQLCipher memory. It does not prove that the gateway's context labels are truthful.

## Measured grounding evals

The original failed report, [`eval/hermes_turns.json`](eval/hermes_turns.json), remains unchanged. The repaired report is [`eval/hermes_turns-2026-10-20.json`](eval/hermes_turns-2026-10-20.json). Both use the same 10 prompts, three runs per prompt, and the same expected-first-tool decision rule in the real container.

| Receipt and candidate | Grounded turns | Hosted tokens | Total wall latency | Computed cost | Security |
|---|---:|---:|---:|---:|---|
| Original Lightning, failed | 12 of 30 | 220,827 | 287,894 ms | $0.01402236 | 0 SSN egress violations; 3 of 3 injection probes passed |
| Original Super, failed | 11 of 30 | 147,611 | 190,277 ms | $0.04649430 | 0 SSN egress violations; 3 of 3 injection probes passed |
| Repaired Hermes with Lightning, passed | 27 of 30 | 178,264 | 86,958 ms | $0.01134600 | 0 SSN egress violations; 3 of 3 injection probes passed |
| Repaired Super | Not run | Not run | Not run | $0 | Lightning had already passed, so the fixed decision rule stopped |
| Built-in Rust comparison, failed | 0 of 30 | 28,809 | 102,655 ms | $0.01061010 | 0 SSN egress violations; 0 of 3 Hermes-specific probes passed |

The original failure had two gateway compatibility causes. Hermes called `read_document` with the well-formed value `{"name":"W-2"}`, while the handler accepted only `w2`. Separately, `get_consent_state` returned an array in MCP `structuredContent`, but the negotiated MCP 2025 schema requires an object. Repeated validation failures opened Hermes's server circuit breaker. The fix accepts only the exact `w2` and observed `W-2` document identifiers, returns consent state inside an object, and keeps malformed requests and unrelated document names refused.

The required threshold is 27 of 30. Lightning passed exactly, so the shipped keyed-container default is `PRU_AGENT=hermes` with `nvidia/Nemotron-3_5-Lightning`; Super was not rerun. The built-in loop completed without new infrastructure, but it exposes different tool names and lacks the read and note tools, so it scored 0 under the unchanged Hermes rule. Its 0 of 3 security probes means it did not create the Hermes-specific Token Factory refusal rows those probes require; its SSN turns stayed local and the receipt recorded 0 SSN egress violations.

The repaired eval cost $0.02195610. Including the $0.00725100 diagnostic reproduction, cumulative computed spend was $0.02920710, below the $0.50 cap. Costs use returned usage and code-configured rates of $0.06/$0.24 per million Lightning input/output tokens and $0.30/$0.90 for Super. Those rates are NOT VERIFIED against an invoice.

## Measured SSN canary result

The committed report is [`eval/ssn_canaries.json`](eval/ssn_canaries.json).

| Measure | Result |
|---|---:|
| Positive synthetic canaries | 360 |
| True positives | 360 |
| Misses | 0 |
| Measured recall | 1.000000 |
| Negative controls | 120 |
| Flagged negative controls | 0 |
| False-positive spans | 0 |
| One-sided 95% upper bound on miss rate | 0.008286950875235621 |

The positive generator uses only group `00` or area `9xx` for full identifiers. The Social Security Administration does not issue either range. The masked cases expose only a generated four-digit suffix. Negative controls include synthetic phone numbers, EINs, ZIP+4 values, dates, unlabeled account numbers, and OCR-like prose.

## Workspace

| Crate | Responsibility |
|---|---|
| `pru-ssn` | Span recognition and deterministic canary evaluation |
| `pru-consent` | Consent records, salted PIN hashes, Biscuit minting and verification |
| `pru-policy` | Cedar authorization and SymCC policy implication tests |
| `pru-boundary` | Consent-ledger to OpenShell REST boundary compiler |
| `pru-gateway` | Axum action boundary, Hermes model provider, MCP tools, route selection, notes, dry runs, and egress ledger |
| `pru-agent-jail` | Linux Landlock launcher for the uid 10000 Hermes process |

## Run the Rust gates

Use Rust 1.89.0, which is pinned in `rust-toolchain.toml`.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p pru-ssn --bin generate-ssn-eval -- eval/ssn_canaries.json
git diff --exit-code -- eval/ssn_canaries.json
```

## Run the Cedar proof

On Windows, the wrapper downloads cvc5 1.3.1, verifies its SHA-256 checksum, sets `CVC5`, and runs the SymCC tests.

```powershell
.\scripts\run-symcc.ps1
```

Linux and GitHub Actions use the matching pinned installer and proof script.

```bash
bash scripts/install-cvc5.sh
bash scripts/run-symcc.sh
```

## Compile and prove OpenShell boundaries

Regenerate the committed synthetic example:

```powershell
cargo run -p pru-boundary -- eval/ledgers/synthetic-client.json eval/boundaries/synthetic-client.yaml
```

Run prover 0.1.2 in the configured WSL distro:

```powershell
.\scripts\prove-boundaries-wsl.ps1
```

The Linux script treats exit code 3, which means unsupported or inconclusive, as a hard failure. It also plants an extra host and requires the prover to return exit code 1 with an `exceeds_boundary` result.

## Run the gateway tests

```powershell
cargo test -p pru-gateway -- --nocapture
```

The tests start local HTTP stubs only inside the test process. They verify local routing for an SSN canary, Token Factory routing for a consented SSN-free prompt, denial before upstream contact, dry-run email and calendar behavior, and absence of raw SSNs in the ledger.

## Run one live Token Factory call

Set `NEBIUS_API_KEY` in the environment. Do not write it into the repository. Generate a short-lived synthetic disclosure consent and keep the output in a temporary file. Start the gateway with the public key, a random 32-byte ledger hash key, and the same environment key. Then export the generated token as `PRU_CONSENT_TOKEN` and run:

```bash
bash scripts/live-llm.sh
```

The script sends only this synthetic prompt: `This is a synthetic test. Reply with the sum of 17 and 23.` It prints the gateway decision, route, gateway latency, and wall latency. It does not print `NEBIUS_API_KEY`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
