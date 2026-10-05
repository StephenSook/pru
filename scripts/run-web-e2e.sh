#!/usr/bin/env bash
set -euo pipefail

run_dir="${RUNNER_TEMP:-/tmp}/pru-e2e-${GITHUB_RUN_ID:-local}"
mkdir -p "$run_dir/data"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  for process_id in "${web_pid:-}" "${gateway_pid:-}" "${mock_pid:-}"; do
    if [[ -n "$process_id" ]]; then
      kill "$process_id" 2>/dev/null || true
    fi
  done
  wait 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT INT TERM

node scripts/mock-openai.mjs >"$run_dir/mock.log" 2>&1 &
mock_pid=$!

PRU_BIND=127.0.0.1:8787 \
PRU_DATA_DIR="$run_dir/data" \
PRU_LEDGER_HASH_KEY=1111111111111111111111111111111111111111111111111111111111111111 \
PRU_LOCAL_LLM_URL=http://127.0.0.1:8082/v1 \
PRU_TOKEN_FACTORY_URL=http://127.0.0.1:8082/v1 \
NEBIUS_API_KEY=ci-test-key \
target/debug/pru-gateway >"$run_dir/gateway.log" 2>&1 &
gateway_pid=$!

PRU_GATEWAY_URL=http://127.0.0.1:8787 pnpm --dir web start >"$run_dir/web.log" 2>&1 &
web_pid=$!

wait_for() {
  local name="$1"
  local url="$2"
  local process_id="$3"
  for _ in $(seq 1 60); do
    if curl --fail --silent "$url" >/dev/null; then
      return 0
    fi
    if ! kill -0 "$process_id" 2>/dev/null; then
      echo "$name stopped before readiness" >&2
      return 1
    fi
    sleep 1
  done
  echo "$name did not become ready within 60 seconds" >&2
  return 1
}

wait_for "mock OpenAI server" http://127.0.0.1:8082/health "$mock_pid"
wait_for "gateway" http://127.0.0.1:8787/healthz "$gateway_pid"
wait_for "web" http://127.0.0.1:3000/healthz "$web_pid"

pnpm --dir web e2e
