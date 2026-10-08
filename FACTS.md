# Pru code-audited facts

This file is the source for README claims, the Devpost writeup, and demo video narration. Every factual row carries a plain-language claim, an exact value, a source, and a tag. MEASURED rows whose source is a committed JSON or TOML file plus key use `json:path#key` or `toml:path#key` so `crates/pru-ssn/tests/facts.rs` can compare them with the artifact. Exact values for those rows are JSON literals inside backticks.

Pru is the Personal AI track entry. The description below is from the shipped code.

## What Pru does

Pru is a tax-practice assistant for a solo preparer or small practice. A Rust gateway is the only permitted egress path. It scans action arguments for SSN-shaped spans, verifies a signed consent token, asks Cedar, and writes a JSONL decision ledger of keyed span hashes. SSN-bearing prompts stay on the machine and go to a local OpenAI-compatible llama-server. Consented SSN-free prompts may go to Nebius Token Factory. Email and calendar tools build dry-run drafts and do not send. A Next.js judge door talks only to a fixed server route that proxies to the gateway. Demo clients are synthetic, labelled test data, and use SSN-shaped values that the Social Security Administration does not issue.

## Privacy and consent design

Pru must never hold real taxpayer data. This repository uses code-generated synthetic canaries and two synthetic demo clients. The product plan permits IRS public test taxpayers labelled as test data. This repository contains none of those IRS ATS records.

| Claim | Exact value | Source | Tag |
|---|---|---|---|
| Demo clients are labelled test data | `true` | `crates/pru-gateway/src/api.rs:144` and `:155` (`test_data: true` on Avery Morgan and Riley Chen) | MEASURED |
| Avery Morgan synthetic SSN range | `"Group00"` | `crates/pru-gateway/src/api.rs:144` (`synthetic_test_ssn(1, TestSsnRange::Group00)`) | MEASURED |
| Riley Chen synthetic SSN range | `"Area9xx"` | `crates/pru-gateway/src/api.rs:153` (`synthetic_test_ssn(2, TestSsnRange::Area9xx)`) | MEASURED |
| Demo client list view omits the SSN field | `["id","name","email","test_data"]` | `crates/pru-gateway/src/api.rs:120-134` (`DemoClientView` has no `ssn`) | MEASURED |
| Group `00` generator form | `"{area:03}-00-{serial:04}"` with `area = 101 + (index % 565)` and `serial = 1000 + (index % 9000)` | `crates/pru-ssn/src/lib.rs:180-186` | MEASURED |
| Area `9xx` generator form | `"{area:03}-{group:02}-{serial:04}"` with `area = 900 + (index % 100)` and `group = 10 + (index % 80)` | `crates/pru-ssn/src/lib.rs:187-191` | MEASURED |
| Recognizer marks group 00 or area 900+ as test-only | `test_only = group == 0 \|\| area >= 900` | `crates/pru-ssn/src/lib.rs:79` | MEASURED |
| Why those ranges cannot be issued | `"SSA does not issue group 00 or area 900-999"` | [SSA randomization FAQs](https://www.ssa.gov/employer/randomizationfaqs.html); module comment at `crates/pru-ssn/src/lib.rs:3-5` | SOURCED |
| IRS public test taxpayers use group 00 | `"IRS public test taxpayers use group 00; this repo does not contain those records"` | `crates/pru-ssn/src/lib.rs:3-5`; check that would verify IRS ATS records: open an IRS ATS publication and confirm no such records are committed under `eval/` or `crates/` | SOURCED |
| Canary dataset ranges | `["group 00, which SSA does not issue","area 9xx, which SSA does not issue"]` | `json:eval/ssn_canaries.json#dataset.synthetic_identifier_ranges` | MEASURED |
| Consent PIN minimum length | `5` | `crates/pru-consent/src/lib.rs:19-20` (`MINIMUM_PIN_LENGTH`); check at `:401-406` | MEASURED |
| PIN hash algorithm | `"argon2id"` | `crates/pru-consent/src/lib.rs:59-61` and `:94`; test at `:664` asserts `$argon2id$` | MEASURED |
| Judge-door test PIN | `"53179"` | `web/src/ui/JudgeDoor.tsx:7` | MEASURED |
| Judge-door legal note | `"TEST DATA ONLY. Real taxpayer data is barred by 26 U.S.C. 7216."` | `web/src/ui/JudgeDoor.tsx:199` | MEASURED |
| Cedar SSN forbid | `resource.contains_ssn && resource.destination_region != "local"` | `crates/pru-policy/cedar/live.cedar:32-39` | MEASURED |
| Cedar use permit requires matching unexpired use consent | `context.consent_kind == "use"` plus client, purpose, and expiry checks | `crates/pru-policy/cedar/live.cedar:2-13` | MEASURED |
| Cedar disclose permit requires matching unexpired disclose consent | `context.consent_kind == "disclose"` plus client, recipient, purpose, and expiry checks | `crates/pru-policy/cedar/live.cedar:17-29` | MEASURED |
| Gateway verifies the Biscuit before Cedar | `consent_verifier.verify(&request.consent_token, now, &revocations)` | `crates/pru-gateway/src/lib.rs:428-432` | MEASURED |
| Gateway asks Cedar after consent verification | `authorizer.authorize(&policy_request)` | `crates/pru-gateway/src/lib.rs:514-518` | MEASURED |
| SSN-bearing LLM actions stay local | `"local"` | `crates/pru-gateway/src/lib.rs:392-397` | MEASURED |
| Ledger stores keyed Blake3 hashes of matched spans | `blake3::keyed_hash(&self.hash_key, span.matched.as_bytes())` | `crates/pru-gateway/src/lib.rs:230-238` | MEASURED |
| Chat answers redact detected SSNs | `"[SSN kept local]"` | `crates/pru-gateway/src/chat.rs:380-387` | MEASURED |
| Email and calendar remain dry-run | `"sent": false` | `crates/pru-gateway/tests/gateway.rs:293-295` (`response.dry_run` and `result["sent"] == false`) | MEASURED |
| Next.js proxy allowlist | `v1/demo/(clients\|reset)`, `v1/clients/{id}/(chat\|ledger\|consents)`, revoke by UUID | `web/src/app/api/pru/[...path]/route.ts:6-11` | MEASURED |
| Next.js proxy max body | `16384` | `web/src/app/api/pru/[...path]/route.ts:4` | MEASURED |
| Next.js proxy does not forward browser credentials | no `authorization` or `cookie` on the gateway `fetch` | `web/src/app/api/pru/[...path]/route.ts:34-38` | MEASURED |
| llama-server starts without the Token Factory key | `env -u NEBIUS_API_KEY /opt/llama/llama-server` | `docker/entrypoint.sh:27` | MEASURED |
| Next.js starts after the Token Factory key is unset | `unset NEBIUS_API_KEY` then `"$@"` | `docker/entrypoint.sh:72-74` | MEASURED |
| Judge door fetches only `/api/pru/` | `fetch(\`/api/pru/${path}\`)` | `web/src/ui/JudgeDoor.tsx:15` (`api` helper) | MEASURED |

## Models and services

| Claim | Exact value | Source | Tag |
|---|---|---|---|
| Token Factory model id | `"nvidia/Nemotron-3_5-Lightning"` | `crates/pru-gateway/src/lib.rs:32` (`TOKEN_FACTORY_MODEL`) | MEASURED |
| Token Factory recipient label | `"Nebius Token Factory"` | `crates/pru-gateway/src/lib.rs:33` | MEASURED |
| Token Factory base URL default | `"https://api.tokenfactory.nebius.com/v1"` | `crates/pru-gateway/src/main.rs:38-39`; `.env.example:10` | MEASURED |
| Local llama-server URL default | `"http://127.0.0.1:8081/v1"` | `crates/pru-gateway/src/main.rs:33-34` | MEASURED |
| Local model alias | `"nemotron-3-nano-4b"` | `crates/pru-gateway/src/main.rs:35-36`; `Dockerfile:65` | MEASURED |
| Local GGUF file | `"NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf"` | `Dockerfile:5` (`MODEL_FILE`) | MEASURED |
| Hugging Face repository | `"nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF"` | `Dockerfile:49` | MEASURED |
| Hugging Face revision | `"ba223d14e45525f7fae81db77ea8cabeb2fc6c25"` | `Dockerfile:4` (`MODEL_REVISION`) | MEASURED |
| GGUF SHA-256 | `"be5d9a656a51922f24f1f09a759cebb694e1f5d9728bf0ef9f8c972c5a0b5ef2"` | `Dockerfile:6` (`MODEL_SHA256`) | MEASURED |
| GGUF file size, bytes | `2837072864` | command: `stat -c '%s' /mnt/d/hf-cache/gguf/nvidia-NVIDIA-Nemotron-3-Nano-4B-GGUF/NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf`; output: `2837072864`; README table lists `2,837,072,864` | MEASURED |
| llama.cpp pin | `"a7fb71fab83b474a0892b9a05aaa3a8ddca2729b"` | `Dockerfile:3` (`LLAMA_CPP_COMMIT`) | MEASURED |
| llama.cpp tag for that commit | `"b11401"` | GitHub REST `GET https://api.github.com/repos/ggml-org/llama.cpp/git/refs/tags/b11401`; output: `sha=a7fb71fab83b474a0892b9a05aaa3a8ddca2729b` | SOURCED |
| Container llama-server GPU layers | `0` | `docker/entrypoint.sh:36` (`--n-gpu-layers 0`) | MEASURED |
| Gateway bind default | `"127.0.0.1:8787"` | `crates/pru-gateway/src/main.rs:52`; `Dockerfile:62` | MEASURED |
| Chat list-price formula | `(input * 0.06 + output * 0.24) / 1_000_000` | `crates/pru-gateway/src/chat.rs:372-377`; this is a code constant, not a live Nebius invoice | MEASURED |
| Chat test cost for the Token Factory stub | `0.0000108` | `crates/pru-gateway/tests/chat_api.rs:210` | MEASURED |
| OpenShell compiler target | `"OpenShell 0.0.116 and prover 0.1.2"` | `crates/pru-boundary/src/lib.rs:35` | MEASURED |
| Compiled boundary host | `"pru-gateway.local:443"` | `eval/boundaries/synthetic-client.yaml:32-34` | MEASURED |
| Compiled YAML names Hermes paths | `"/opt/hermes"` and `"/usr/local/bin/hermes"` | `eval/boundaries/synthetic-client.yaml:7,47`; template at `crates/pru-boundary/src/lib.rs:57,` binary path in the same format string | MEASURED |
| Compiled YAML names NemoClaw paths | `"/run/nemoclaw/managed-startup-ca-bundle.pem"` | `eval/boundaries/synthetic-client.yaml:11-13`; template at `crates/pru-boundary/src/lib.rs:61-63` | MEASURED |
| Hermes Agent in the product runtime | `"NOT WIRED"` | no `hermes` binary spawn, no NemoClaw onboard, no OpenShell sandbox start in `crates/` or `docker/entrypoint.sh`; YAML paths are a compile template | MEASURED |
| NemoClaw in the product runtime | `"NOT WIRED"` | same grep: no `nemoclaw` or `nemohermes` call in product code | MEASURED |
| OpenShell sandbox in the product runtime | `"NOT WIRED"` | `pru-boundary` writes YAML; `scripts/prove-boundaries.sh` runs the standalone prover; the gateway does not start OpenShell | MEASURED |
| Tavily | `"NOT WIRED"` | workspace grep for `Tavily`/`tavily` returned no matches | MEASURED |
| Nebius Token Factory HTTP call | `POST {base}/chat/completions` | `crates/pru-gateway/src/lib.rs:626-628` | MEASURED |
| Other Nebius services (Serverless GPU jobs, Token Factory list-price API) | `"NOT WIRED"` | no job-submit or billing client in `crates/` or `web/` | MEASURED |

Spike notes live in the sibling gitignored tree `../pru/spike/` (this repository's `.gitignore` excludes `spike/`). They are not product wiring.

| Claim | Exact value | Source | Tag |
|---|---|---|---|
| Spike llama.cpp GPU generation (2026-10-04) | `"about 95 tok/s generation"` | `../pru/spike/WSL-SPIKE.md:13` | MEASURED |
| Spike Hermes local cold answer (2026-10-05 retry) | `"391 in 12.70 s"` | `../pru/spike/WSL-SPIKE.md:251` | MEASURED |
| Spike Hermes local warm answer | `"Mercury in 1.20 s"` | `../pru/spike/WSL-SPIKE.md:252` | MEASURED |
| Spike Hermes Token Factory round trip | `"391 in 1.62 s, model nvidia/Nemotron-3_5-Lightning"` | `../pru/spike/WSL-SPIKE.md:268` | MEASURED |

## Measured results

| Claim | Exact value | Source | Tag |
|---|---|---|---|
| Workspace version | `"0.1.0"` | `toml:Cargo.toml#workspace.package.version` | MEASURED |
| Workspace license | `"Apache-2.0"` | `toml:Cargo.toml#workspace.package.license` | MEASURED |
| Rust version | `"1.89"` | `toml:Cargo.toml#workspace.package.rust-version` | MEASURED |
| Toolchain channel | `"1.89.0"` | `toml:rust-toolchain.toml#toolchain.channel` | MEASURED |
| Web package name | `"pru-judge-door"` | `json:web/package.json#name` | MEASURED |
| Web package version | `"0.1.0"` | `json:web/package.json#version` | MEASURED |
| Web package manager | `"pnpm@10.17.1"` | `json:web/package.json#packageManager` | MEASURED |
| Next.js version | `"16.3.6"` | `json:web/package.json#dependencies.next` | MEASURED |
| Canary schema version | `1` | `json:eval/ssn_canaries.json#schema_version` | MEASURED |
| Positive synthetic canaries | `360` | `json:eval/ssn_canaries.json#dataset.positive_canaries` | MEASURED |
| Negative controls | `120` | `json:eval/ssn_canaries.json#dataset.negative_controls` | MEASURED |
| True positives | `360` | `json:eval/ssn_canaries.json#results.true_positives` | MEASURED |
| Misses | `0` | `json:eval/ssn_canaries.json#results.misses` | MEASURED |
| Measured recall | `1.0` | `json:eval/ssn_canaries.json#results.recall` | MEASURED |
| False-positive spans | `0` | `json:eval/ssn_canaries.json#results.false_positive_spans` | MEASURED |
| Negative controls flagged | `0` | `json:eval/ssn_canaries.json#results.negative_controls_flagged` | MEASURED |
| False-positive rate per control | `0.0` | `json:eval/ssn_canaries.json#results.false_positive_rate_per_control` | MEASURED |
| One-sided 95% miss-rate upper bound | `0.008286950875235621` | `json:eval/ssn_canaries.json#results.miss_rate_upper_bound_95_one_sided` | MEASURED |
| Confidence method | `"exact Clopper-Pearson binomial bound"` | `json:eval/ssn_canaries.json#results.confidence_method` | MEASURED |
| Dashed canary count | `60` | `json:eval/ssn_canaries.json#format_counts.dashed` | MEASURED |
| OCR-noise canary count | `60` | `json:eval/ssn_canaries.json#format_counts.ocr_noise` | MEASURED |
| Partially masked canary count | `60` | `json:eval/ssn_canaries.json#format_counts.partially_masked` | MEASURED |
| Plain-in-context canary count | `60` | `json:eval/ssn_canaries.json#format_counts.plain_in_context` | MEASURED |
| Spaced canary count | `60` | `json:eval/ssn_canaries.json#format_counts.spaced` | MEASURED |
| Split-across-line-break canary count | `60` | `json:eval/ssn_canaries.json#format_counts.split_across_line_break` | MEASURED |
| Synthetic ledger client id | `"synthetic-client-001"` | `json:eval/ledgers/synthetic-client.json#client_id` | MEASURED |
| Canaries per format in the generator | `60` | `crates/pru-ssn/src/lib.rs:203` (`CASES_PER_FORMAT`) | MEASURED |
| Negative controls per kind in the generator | `20` | `crates/pru-ssn/src/lib.rs:204` (`NEGATIVE_CASES_PER_KIND`) | MEASURED |
| OpenShell prover version used here | `"openshell-prover 0.1.2"` | command: `$HOME/.local/opt/openshell-prover-0.1.2/openshell-prover --version`; output: `openshell-prover 0.1.2` | MEASURED |
| Compiled boundary self-check | `"within_boundary, exit 0"` | command: `OPENSHELL_PROVER=$HOME/.local/opt/openshell-prover-0.1.2/openshell-prover bash scripts/prove-boundaries.sh`; output: `case=compiled_boundary_self file=synthetic-client.yaml result: within_boundary exit=0` | MEASURED |
| Planted extra host | `"exceeds_boundary, host=example.com:443, exit 1"` | same command; output: `case=planted_extra_host expected=exceeds_boundary result: exceeds_boundary host=example.com:443 exit=1`; script overall `EXIT=0` | MEASURED |
| Cedar live policy implies reference R | `"NOT VERIFIED on this machine"` | Check: `bash scripts/install-cvc5.sh` then `bash scripts/run-symcc.sh` on a host with cvc5 1.3.1 and Rust 1.89. Windows has no `cvc5.exe`. WSL has cvc5 1.3.1 and no `rustc`. Tests are `crates/pru-policy/tests/symcc.rs:47` and `:59` (`required-features = ["symcc"]`) | NOT VERIFIED |
| cvc5 pin | `"1.3.1"` | `scripts/install-cvc5.sh:5`; checksum `1a1cda20d2df4938fa4944a69f33ddc9172e319ece0eed0aa09c4d7abede3ed1` at line 7 | MEASURED |
| cedar-policy-symcc pin | `"0.7.0"` | `crates/pru-policy/Cargo.toml:19` | MEASURED |
| CPU Docker first-token times from README | `"19.1551 / 11.7070 / 8.3718 s"` | README.md:57-59; no committed receipt under `eval/` | NOT VERIFIED |
| CPU Docker output speed from README | `"2.9792 / 0.9256 / 3.4185 tokens/s"` | README.md:57-59; check: `docker exec pru-phase-b1 node /app/benchmark-local-model.mjs` | NOT VERIFIED |
| Chat max input characters | `4000` | `crates/pru-gateway/src/chat.rs:12` | MEASURED |
| Chat max model calls | `4` | `crates/pru-gateway/src/chat.rs:13` | MEASURED |
| Chat max tool calls | `4` | `crates/pru-gateway/src/chat.rs:14` | MEASURED |
| Chat max output tokens | `2000` | `crates/pru-gateway/src/chat.rs:15` | MEASURED |

## Tests and CI

CI workflow `.github/workflows/ci.yml` defines five jobs: `quality`, `formal-proofs`, `web`, `web-e2e`, `container-build`.

`quality` steps: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo run -p pru-ssn --bin generate-ssn-eval -- eval/ssn_canaries.json`; `git diff --exit-code -- eval/ssn_canaries.json`.

`formal-proofs` steps: `bash scripts/install-cvc5.sh`; `bash scripts/run-symcc.sh`; `bash scripts/prove-boundaries.sh`.

`web` steps (in `web/`): `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm lint`; `pnpm test`; `pnpm build`. pnpm 10.17.1, Node 22.

`web-e2e` steps: `cargo build --locked -p pru-gateway`; `pnpm --dir web install --frozen-lockfile`; `pnpm --dir web build`; Playwright Chromium; `bash scripts/run-web-e2e.sh`.

`container-build` steps: free runner disk; `docker/build-push-action` with `push: false`. The image carries the GGUF and compiles llama.cpp.

Tracked `#[test]` / `#[tokio::test]` count before the FACTS parser test: 48, of which 2 are `crates/pru-policy/tests/symcc.rs` with `required-features = ["symcc"]` and are skipped by default `cargo test --workspace`. Default `cargo test --workspace` on this machine: 46 passed (2+2+8+4+6+6+5+5+8 across the crates that define tests).

Web unit tests: 3 vitest cases in `web/tests/api-route.test.ts`. Playwright: 1 consent-lifecycle test, 1 axe test, 4 overflow widths (`390`, `768`, `1024`, `1440`) in `web/tests/e2e/judge-door.spec.ts`. Playwright timeout `180000` ms at `web/playwright.config.ts:5`.

Local gate results (Windows `cargo.exe` / `pnpm`, this session):

| Claim | Exact value | Source | Tag |
|---|---|---|---|
| quality fmt | `0` | command: `cargo.exe fmt --all -- --check`; output: `EXIT_FMT=0` | MEASURED |
| quality clippy | `0` | command: `cargo.exe clippy --workspace --all-targets --offline -- -D warnings`; output: `EXIT_CLIPPY=0` | MEASURED |
| quality cargo test | `0` | command: `cargo.exe test --workspace --offline`; output: `EXIT_TEST=0` and 46 passed | MEASURED |
| quality generate-ssn-eval | `0` | command: `cargo.exe run --offline -p pru-ssn --bin generate-ssn-eval -- eval/ssn_canaries.json`; output: `EXIT_EVAL=0` `positives=360 true_positives=360 misses=0 recall=1.000000` | MEASURED |
| quality canary diff | `0` | command: `git.exe diff --exit-code -- eval/ssn_canaries.json`; output: `EXIT_DIFF=0` | MEASURED |
| web typecheck | `0` | command: `pnpm typecheck` in `web/`; output: `EXIT_TYPECHECK=0` | MEASURED |
| web lint | `0` | command: `pnpm lint` in `web/`; output: `EXIT_LINT=0` | MEASURED |
| web vitest | `"3 passed"` | command: `pnpm test` in `web/`; output: `Tests  3 passed (3)` `EXIT_TEST=0` | MEASURED |
| web Next.js build | `0` | command: `pnpm build` in `web/`; output: `Next.js 16.3.6` `EXIT_BUILD=0` | MEASURED |
| OpenShell prove-boundaries | `0` | command: `bash scripts/prove-boundaries.sh` with prover 0.1.2; output: overall `EXIT=0` | MEASURED |

Gates that could not run here:

| Gate | Why it did not run here |
|---|---|
| `formal-proofs` SymCC | Windows has no `cvc5.exe`. WSL has cvc5 1.3.1 at `~/.local/opt/cvc5-1.3.1` and no `rustc`. The installer would download cvc5, which this session forbids except for GitHub REST. |
| `container-build` | Docker build fetches Hugging Face weights and llama.cpp; this session forbids those network calls and forbids deploy. |
| `web-e2e` | CI script `scripts/run-web-e2e.sh` expects a Linux `target/debug/pru-gateway`. This session compiled with Windows `cargo.exe`, so the binary is `target/debug/pru-gateway.exe`. WSL has no `rustc` to rebuild. Vitest and `pnpm build` already passed. |

## Do not claim

- A tax practitioner has used Pru on a real engagement.
- Pru handles real taxpayer returns or stores real SSNs.
- A public live demo URL exists. The judge door binds to `http://127.0.0.1:3000` (`web/playwright.config.ts:8`, README Phase B).
- Tavily is part of Pru. There is no Tavily call in this repository.
- Hermes Agent runs inside NVIDIA OpenShell through NemoClaw in the product. README.md:3 says that. Phase A at README.md:20 and Phase B at README.md:68 say Hermes is not wired. Product code agrees with the later sentences.
- The OpenShell YAML template means Hermes or NemoClaw execute in the container. They do not.
- The chat `cost_usd` figure is a live Nebius Token Factory invoice. It is the list-price formula in `crates/pru-gateway/src/chat.rs:376`.
- The CPU Docker benchmark numbers in README.md:57-60 were re-measured in this session.
- IRS ATS public test taxpayers are committed in this repository. Only synthetic canaries and two demo clients are.

## Drift found

1. README.md:3 states "Hermes Agent runs inside NVIDIA OpenShell through NemoClaw." README.md:20 (Phase A) and README.md:68 (Phase B) state that Hermes is not wired. Grep of product code finds no Hermes or NemoClaw runtime call. The later README sentences and the code win. Do not use the opening sentence.

No other numeric drift was found between README tables and the committed sources opened for this sheet. The GGUF size `2837072864` matches README `2,837,072,864`. The llama.cpp commit matches GitHub tag `b11401`. The canary table matches `eval/ssn_canaries.json`.
