#!/bin/sh
set -eu

# Visitor workspaces hold test client records. Files the gateway writes are
# readable by their owner only, so another user in the container cannot read them.
umask 077
if [ -n "${PRU_DATA_DIR:-}" ]; then
  mkdir -p "${PRU_DATA_DIR}"
  chmod 0700 "${PRU_DATA_DIR}"
fi

if [ -z "${NEBIUS_API_KEY:-}" ]; then
  echo "Token Factory disabled"
fi

PRU_TOKEN_FACTORY_MODEL="${PRU_TOKEN_FACTORY_MODEL:-nvidia/nemotron-3-super-120b-a12b}"
case "${PRU_TOKEN_FACTORY_MODEL}" in
  nvidia/Nemotron-3_5-Lightning|nvidia/nemotron-3-super-120b-a12b) ;;
  *)
    echo "PRU_TOKEN_FACTORY_MODEL is not an evaluated model" >&2
    exit 1
    ;;
esac
export PRU_TOKEN_FACTORY_MODEL

if [ -z "${PRU_LEDGER_HASH_KEY:-}" ]; then
  PRU_LEDGER_HASH_KEY="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  export PRU_LEDGER_HASH_KEY
fi

if [ -z "${PRU_AGENT_SECRET:-}" ]; then
  PRU_AGENT_SECRET="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  export PRU_AGENT_SECRET
fi

if [ -z "${PRU_HERMES_API_KEY:-}" ]; then
  PRU_HERMES_API_KEY="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  export PRU_HERMES_API_KEY
fi

PRU_AGENT_REQUIRE_JAIL="${PRU_AGENT_REQUIRE_JAIL:-1}"
export PRU_AGENT_REQUIRE_JAIL
case "${PRU_AGENT_REQUIRE_JAIL}" in
  0|1) ;;
  *)
    echo "PRU_AGENT_REQUIRE_JAIL must be 0 or 1" >&2
    exit 1
    ;;
esac

hermes_enabled=0
hermes_jailed=0
case "${PRU_AGENT:-hermes}" in
  builtin)
    PRU_AGENT_LABEL="built-in Rust loop"
    PRU_AGENT_JAIL_STATUS="not applicable"
    ;;
  hermes)
    if [ -z "${NEBIUS_API_KEY:-}" ]; then
      PRU_AGENT=builtin
      PRU_AGENT_LABEL="built-in Rust loop (no Token Factory key)"
      PRU_AGENT_JAIL_STATUS="not applicable"
    else
      jail_probe=""
      if jail_probe="$(setpriv --reuid=10000 --regid=10000 --clear-groups pru-agent-jail --probe 2>&1)"; then
        jail_abi="${jail_probe#landlock_abi=}"
        jail_abi="${jail_abi%% *}"
        hermes_enabled=1
        hermes_jailed=1
        PRU_AGENT_LABEL="Hermes Agent v0.22.2 (Nous Research)"
        PRU_AGENT_JAIL_STATUS="Landlock enforced (ABI ${jail_abi})"
      elif [ "${PRU_AGENT_REQUIRE_JAIL}" = "1" ]; then
        PRU_AGENT=builtin
        PRU_AGENT_LABEL="built-in Rust loop (Landlock unavailable)"
        PRU_AGENT_JAIL_STATUS="required but unavailable"
        echo "Hermes disabled because the Landlock ABI 4 jail could not be enforced: ${jail_probe}" >&2
      else
        hermes_enabled=1
        PRU_AGENT_LABEL="Hermes Agent v0.22.2 (Nous Research)"
        PRU_AGENT_JAIL_STATUS="not enforced (operator override)"
        echo "Warning: starting Hermes without Landlock because PRU_AGENT_REQUIRE_JAIL=0" >&2
      fi
    fi
    ;;
  *)
    echo "PRU_AGENT must be builtin or hermes" >&2
    exit 1
    ;;
esac
export PRU_AGENT PRU_AGENT_LABEL PRU_AGENT_JAIL_STATUS

if [ "${hermes_enabled}" = "1" ]; then
  install -d -m 0700 -o 10000 -g 10000 "${HERMES_HOME}" "${HERMES_RUNTIME_DIR}"
  sed "s|^  default: .*$|  default: ${PRU_TOKEN_FACTORY_MODEL}|" \
    /etc/pru/hermes-config.yaml > "${HERMES_HOME}/config.yaml"
  chown 10000:10000 "${HERMES_HOME}/config.yaml"
  chmod 0600 "${HERMES_HOME}/config.yaml"
fi

PRU_LOCAL_THREADS="${PRU_LOCAL_THREADS:-$(nproc)}"
export PRU_LOCAL_THREADS

cleanup() {
  status=$?
  trap - EXIT INT TERM
  for process_id in "${web_pid:-}" "${hermes_pid:-}" "${gateway_pid:-}" "${llama_pid:-}"; do
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
  --threads "${PRU_LOCAL_THREADS}" \
  --threads-batch "${PRU_LOCAL_THREADS}" \
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

if [ "${hermes_enabled}" = "1" ]; then
  if [ "${hermes_jailed}" = "1" ]; then
    setpriv --reuid=10000 --regid=10000 --clear-groups \
      env -i \
        API_SERVER_ENABLED=true \
        API_SERVER_HOST=127.0.0.1 \
        API_SERVER_KEY="${PRU_HERMES_API_KEY}" \
        API_SERVER_MODEL_NAME=hermes-agent \
        API_SERVER_PORT=8642 \
        HERMES_HOME="${HERMES_HOME}" \
        HERMES_PYTHON=/opt/hermes/.venv/bin/python \
        HERMES_RUNTIME_DIR="${HERMES_RUNTIME_DIR}" \
        HOME="${HERMES_HOME}" \
        LD_LIBRARY_PATH=/usr/local/lib \
        PATH=/opt/hermes/bin:/opt/hermes/.venv/bin:/opt/hermes/tools/python-3.14.7+20260901-linux-x64/bin:/usr/local/bin:/usr/bin:/bin \
        PRU_AGENT_SECRET="${PRU_AGENT_SECRET}" \
        PYTHONDONTWRITEBYTECODE=1 \
        PYTHONUNBUFFERED=1 \
        XDG_RUNTIME_DIR="${HERMES_RUNTIME_DIR}" \
        pru-agent-jail \
          --write-dir "${HERMES_HOME}" \
          --write-dir "${HERMES_RUNTIME_DIR}" \
          --connect-port 8787 \
          -- \
          /opt/hermes/.venv/bin/hermes gateway run --no-supervise --external-supervisor &
  else
    setpriv --reuid=10000 --regid=10000 --clear-groups \
      env -i \
        API_SERVER_ENABLED=true \
        API_SERVER_HOST=127.0.0.1 \
        API_SERVER_KEY="${PRU_HERMES_API_KEY}" \
        API_SERVER_MODEL_NAME=hermes-agent \
        API_SERVER_PORT=8642 \
        HERMES_HOME="${HERMES_HOME}" \
        HERMES_PYTHON=/opt/hermes/.venv/bin/python \
        HERMES_RUNTIME_DIR="${HERMES_RUNTIME_DIR}" \
        HOME="${HERMES_HOME}" \
        LD_LIBRARY_PATH=/usr/local/lib \
        PATH=/opt/hermes/bin:/opt/hermes/.venv/bin:/opt/hermes/tools/python-3.14.7+20260901-linux-x64/bin:/usr/local/bin:/usr/bin:/bin \
        PRU_AGENT_SECRET="${PRU_AGENT_SECRET}" \
        PYTHONDONTWRITEBYTECODE=1 \
        PYTHONUNBUFFERED=1 \
        XDG_RUNTIME_DIR="${HERMES_RUNTIME_DIR}" \
        /opt/hermes/.venv/bin/hermes gateway run --no-supervise --external-supervisor &
  fi
  hermes_pid=$!

  attempt=0
  until curl --fail --silent \
    --header "Authorization: Bearer ${PRU_HERMES_API_KEY}" \
    http://127.0.0.1:8642/health >/dev/null; do
    attempt=$((attempt + 1))
    if ! kill -0 "${hermes_pid}" 2>/dev/null; then
      echo "Hermes stopped before it became ready" >&2
      exit 1
    fi
    if [ "${attempt}" -ge 60 ]; then
      echo "Hermes did not become ready within 60 seconds" >&2
      exit 1
    fi
    sleep 1
  done
fi

unset NEBIUS_API_KEY PRU_AGENT_SECRET PRU_HERMES_API_KEY
"$@" &
web_pid=$!
wait "${web_pid}"
