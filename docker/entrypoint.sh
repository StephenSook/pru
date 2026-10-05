#!/bin/sh
set -eu

if [ -z "${NEBIUS_API_KEY:-}" ]; then
  echo "NEBIUS_API_KEY is required" >&2
  exit 1
fi

if [ -z "${PRU_LEDGER_HASH_KEY:-}" ]; then
  PRU_LEDGER_HASH_KEY="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  export PRU_LEDGER_HASH_KEY
fi

cleanup() {
  status=$?
  trap - EXIT INT TERM
  for process_id in "${web_pid:-}" "${gateway_pid:-}" "${llama_pid:-}"; do
    if [ -n "${process_id}" ]; then
      kill "${process_id}" 2>/dev/null || true
    fi
  done
  wait 2>/dev/null || true
  exit "${status}"
}
trap cleanup EXIT INT TERM

env -u NEBIUS_API_KEY /opt/llama/llama-server \
  --model "${PRU_MODEL_PATH}" \
  --alias "${PRU_LOCAL_LLM_MODEL}" \
  --host 127.0.0.1 \
  --port 8081 \
  --ctx-size 8192 \
  --parallel 1 \
  --threads "${PRU_LOCAL_THREADS:-8}" \
  --threads-batch "${PRU_LOCAL_THREADS:-8}" \
  --n-gpu-layers 0 \
  --jinja \
  --reasoning off \
  --metrics &
llama_pid=$!

attempt=0
until curl --fail --silent http://127.0.0.1:8081/health >/dev/null; do
  attempt=$((attempt + 1))
  if ! kill -0 "${llama_pid}" 2>/dev/null; then
    echo "llama-server stopped before it became ready" >&2
    exit 1
  fi
  if [ "${attempt}" -ge 180 ]; then
    echo "llama-server did not become ready within 180 seconds" >&2
    exit 1
  fi
  sleep 1
done

pru-gateway &
gateway_pid=$!
attempt=0
until curl --fail --silent http://127.0.0.1:8787/healthz >/dev/null; do
  attempt=$((attempt + 1))
  if ! kill -0 "${gateway_pid}" 2>/dev/null; then
    echo "pru-gateway stopped before it became ready" >&2
    exit 1
  fi
  if [ "${attempt}" -ge 30 ]; then
    echo "pru-gateway did not become ready within 30 seconds" >&2
    exit 1
  fi
  sleep 1
done

unset NEBIUS_API_KEY
"$@" &
web_pid=$!
wait "${web_pid}"
