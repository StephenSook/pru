# Pru

Pru is a tax-practice assistant for a solo preparer or small practice. Hermes Agent runs inside NVIDIA OpenShell through NemoClaw. A Rust gateway is the only permitted egress path. The gateway checks each action against a signed consent token and Cedar policy before it can leave the client compartment.

Pru keeps SSN-bearing prompts on the PC and routes them to a local OpenAI-compatible llama-server running NVIDIA Nemotron 3 Nano 4B. It may send consented, SSN-free prompts to NVIDIA Nemotron on Nebius Token Factory. Phase A uses only code-generated synthetic canaries. The product plan permits the IRS's public test taxpayers, labelled as test data, but this repository contains no real taxpayer data.

## What phase A proves

- `pru-ssn` recognizes the required SSN surface forms, including the IRS test group `00`, and labels visible group `00` or area `9xx` identifiers as `test_only`.
- The deterministic canary harness measured 360 detections from 360 generated positive cases and 0 flagged controls from 120 generated negative controls.
- `pru-consent` mints one Ed25519 Biscuit token per consent document. It checks an Argon2id PIN hash, signature, expiry, kind, recipient, purpose, and revocation status.
- `pru-policy` makes the egress decision with Cedar. SymCC and cvc5 prove that the live policy implies reference policy R for both `use` and `disclose` actions.
- The planted Cedar permit does not imply R. SymCC returns a counterexample where the bad policy permits disclosure and R denies it.
- `pru-boundary` emits a REST-only OpenShell policy whose only network host is the Pru gateway. OpenShell prover 0.1.2 reports the compiled policy within its boundary and reports a planted `example.com` rule outside it.
- `pru-gateway` scans every string in each action argument, verifies consent, asks Cedar, writes a JSONL decision ledger, and selects the local or Token Factory route. Its ledger stores keyed span hashes, never raw SSNs.
- Email and calendar endpoints build a dry-run request and ledger entry. They do not send anything in phase A.

## What phase A does not prove

The formal proof covers the Cedar decision logic. It does not prove that the gateway's context labels are truthful. The SSN result is measured recall on this generated canary distribution, not a guarantee over every document or OCR failure. The OpenShell proof covers the prover's REST fragment. Phase A does not wire Hermes, the sandbox, email, calendar, consent UI, or a real practice workflow. Those are phase B tasks.

This is engineering evidence, not a legal opinion about 26 U.S.C. 7216, 26 C.F.R. 301.7216-3, Rev. Proc. 2013-14, or any specific disclosure.

## Phase B part 1

Phase B part 1 adds one judge-facing URL at `http://127.0.0.1:3000`. The browser talks only to a fixed Next.js server route. The server route talks to the Rust gateway on the container loopback interface. The browser never receives `NEBIUS_API_KEY`.

The image starts three processes. Next.js serves the page. The Axum gateway owns consent checks, Cedar decisions, model routing, and the per-client JSONL ledger. A CPU-only llama-server runs NVIDIA Nemotron 3 Nano 4B for SSN-bearing context. SSN-free context with active use consent may go to `nvidia/Nemotron-3_5-Lightning` on Nebius Token Factory. Email and calendar tools remain dry runs.

The two startup clients are TEST DATA. Their generated SSN-shaped values use group `00` or area `9xx`, which the Social Security Administration does not issue. Resetting the demo clears every consent and ledger and recreates the clients in the ephemeral `/tmp/pru-data` directory. Real taxpayer data is barred by 26 U.S.C. 7216.

Build and run the image from PowerShell. The key comes from the environment and is not copied into an image layer.

```powershell
docker build -t pru:phase-b1 .
$env:NEBIUS_API_KEY = [Environment]::GetEnvironmentVariable('NEBIUS_API_KEY', 'User')
docker run --rm --name pru-phase-b1 -p 3000:3000 -e NEBIUS_API_KEY pru:phase-b1
```

Open `http://127.0.0.1:3000`. Stop the container with `docker stop pru-phase-b1`.

The local model layer is pinned and checked during the build:

| Field | Value |
|---|---|
| Hugging Face repository | [`nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF`](https://huggingface.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF) |
| Repository revision | `ba223d14e45525f7fae81db77ea8cabeb2fc6c25` |
| File | `NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf` |
| File size | 2,837,072,864 bytes |
| SHA-256 | `be5d9a656a51922f24f1f09a759cebb694e1f5d9728bf0ef9f8c972c5a0b5ef2` |
| License | [NVIDIA Nemotron Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-nemotron-open-model-license/) |

llama.cpp is pinned to commit `a7fb71fab83b474a0892b9a05aaa3a8ddca2729b`, release `b11401`. The benchmark ran inside the Docker Desktop container on an Intel i7-9700K with eight CPU threads, a 300-token prompt, a forced 150-token answer, temperature 0, and three runs. Each run reported `stop_type: limit` and exactly 300 input plus 150 output tokens.

| Run | First token | Output speed |
|---:|---:|---:|
| 1 | 19.1551 s | 2.9792 tokens/s |
| 2 | 11.7070 s | 0.9256 tokens/s |
| 3 | 8.3718 s | 3.4185 tokens/s |
| Median | 11.7070 s | 2.9792 tokens/s |

Re-run the measurement inside a running container:

```powershell
docker exec pru-phase-b1 node /app/benchmark-local-model.mjs
```

This part does not wire Hermes inside OpenShell on the hosted box, IRS ATS test taxpayers, Gmail or Calendar sending, a Windows Hello signer, or per-client SQLCipher memory. It does not prove that the gateway's context labels are truthful.

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
| `pru-gateway` | Axum action boundary, route selection, dry runs, and egress ledger |

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
