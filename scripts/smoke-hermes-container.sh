#!/usr/bin/env bash
set -euo pipefail

image="${1:-pru:hermes}"
suffix="${RANDOM}-$$"
network="pru-hermes-smoke-${suffix}"
stub="pru-hermes-stub-${suffix}"
app="pru-hermes-app-${suffix}"
workspace="11111111-1111-4111-8111-111111111111"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stub_source="${repo_root}/scripts/fake-token-factory.py"
if command -v cygpath >/dev/null 2>&1; then
  stub_source="$(cygpath -w "${stub_source}")"
fi

cleanup() {
  docker container rm --force "${app}" "${stub}" >/dev/null 2>&1 || true
  docker network rm "${network}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker image inspect "${image}" >/dev/null
docker network create --internal "${network}" >/dev/null
MSYS_NO_PATHCONV=1 docker run --detach \
  --name "${stub}" \
  --network "${network}" \
  --network-alias token-factory \
  --volume "${stub_source}:/stub.py:ro" \
  python:3.12-slim python /stub.py >/dev/null

stub_ready=0
for attempt in $(seq 1 20); do
  if docker exec "${stub}" python -c \
    'import urllib.request; urllib.request.urlopen("http://127.0.0.1:8000/health", timeout=1).read()' \
    >/dev/null 2>&1; then
    stub_ready=1
    break
  fi
  stub_running="$(docker inspect --format '{{.State.Running}}' "${stub}")"
  stub_running="${stub_running//$'\r'/}"
  if [ "${stub_running}" != "true" ]; then
    echo "Token Factory stub exited during startup" >&2
    docker logs "${stub}" >&2
    exit 1
  fi
  sleep 0.2
done
if [ "${stub_ready}" != "1" ]; then
  echo "Token Factory stub did not become ready" >&2
  docker logs "${stub}" >&2
  exit 1
fi

stub_ip="$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "${stub}")"
stub_ip="${stub_ip//$'\r'/}"
if [[ ! "${stub_ip}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Token Factory stub has invalid internal-network address: ${stub_ip}" >&2
  exit 1
fi

docker run --detach \
  --name "${app}" \
  --network "${network}" \
  --cpus 4 \
  --memory 6g \
  --shm-size 512m \
  --env NEBIUS_API_KEY=container-smoke-key \
  --env PRU_TOKEN_FACTORY_URL="http://${stub_ip}:8000/v1" \
  "${image}" >/dev/null

ready=0
for attempt in $(seq 1 300); do
  if health_json="$(docker exec "${app}" curl --fail --silent http://127.0.0.1:3000/healthz 2>/dev/null)"; then
    ready=1
    break
  fi
  app_running="$(docker inspect --format '{{.State.Running}}' "${app}")"
  app_running="${app_running//$'\r'/}"
  if [ "${app_running}" != "true" ]; then
    echo "Pru container exited during startup" >&2
    docker logs "${app}" >&2
    exit 1
  fi
  sleep 1
done
if [ "${ready}" != "1" ]; then
  echo "Pru container did not become healthy within 300 seconds" >&2
  docker logs "${app}" >&2
  exit 1
fi

HEALTH_JSON="${health_json}" node <<'NODE'
const health = JSON.parse(process.env.HEALTH_JSON);
if (health.status !== "ready") throw new Error(`unexpected health: ${process.env.HEALTH_JSON}`);
if (health.agent !== "Hermes Agent v0.22.2 (Nous Research)") throw new Error(`unexpected agent: ${health.agent}`);
if (!/^Landlock enforced \(ABI [4-9][0-9]*\) \+ seccomp socket filter$/.test(health.jail)) throw new Error(`unexpected jail: ${health.jail}`);
NODE

docker exec "${app}" curl --fail-with-body --silent --show-error \
  "http://${stub_ip}:8000/health" >/dev/null

docker exec "${app}" curl --fail-with-body --silent --show-error \
  --request POST \
  --header "x-pru-workspace-id: ${workspace}" \
  http://127.0.0.1:8787/v1/demo/reset

docker exec "${app}" curl --fail-with-body --silent --show-error \
  --request POST \
  --header "content-type: application/json" \
  --header "x-pru-workspace-id: ${workspace}" \
  --data '{"kind":"use","recipient":null,"purpose":"summarize this synthetic tax client","expires_at":"2099-01-01T00:00:00Z","pin":"53179"}' \
  http://127.0.0.1:8787/v1/clients/demo-avery/consents

if ! chat_json="$(docker exec "${app}" curl --fail-with-body --silent --show-error \
  --max-time 120 \
  --request POST \
  --header "content-type: application/json" \
  --header "x-pru-workspace-id: ${workspace}" \
  --data '{"message":"Summarize the selected TEST DATA client"}' \
  http://127.0.0.1:8787/v1/clients/demo-avery/chat)"; then
  echo "Hermes chat failed: ${chat_json}" >&2
  docker logs "${stub}" >&2
  docker logs "${app}" >&2
  exit 1
fi

CHAT_JSON="${chat_json}" node <<'NODE'
const chat = JSON.parse(process.env.CHAT_JSON);
if (chat.agent?.name !== "Hermes Agent" || chat.agent?.version !== "0.22.2") {
  throw new Error(`unexpected chat agent: ${process.env.CHAT_JSON}`);
}
if (chat.answer !== "The TEST DATA client is Avery Morgan.") {
  throw new Error(`unexpected chat answer: ${chat.answer}`);
}
if (!chat.steps?.some((step) => step.tool_name === "get_client_summary")) {
  throw new Error("Hermes did not call get_client_summary");
}
if (chat.steps.filter((step) => step.kind === "model").length !== 2) {
  throw new Error(`expected two model steps: ${process.env.CHAT_JSON}`);
}
NODE

permissions="$(MSYS_NO_PATHCONV=1 docker exec "${app}" stat --format '%a:%u:%g' /tmp/pru-data /tmp/pru-data/${workspace}/demo-avery/client.json)"
if [ "${permissions}" != $'700:0:0\n600:0:0' ]; then
  echo "Unexpected visitor-data permissions: ${permissions}" >&2
  exit 1
fi
MSYS_NO_PATHCONV=1 docker exec --user 10000:10000 "${app}" sh -c \
  "test ! -r /tmp/pru-data/${workspace}/demo-avery/client.json"

MSYS_NO_PATHCONV=1 docker exec --user 10000:10000 "${app}" env -u NEBIUS_API_KEY sh -c '
  found=0
  for status in /proc/[0-9]*/status; do
    uid=$(sed -n "s/^Uid:[[:space:]]*\([0-9]*\).*/\1/p" "${status}")
    [ "${uid}" = "10000" ] || continue
    pid=${status#/proc/}
    pid=${pid%/status}
    [ "${pid}" = "$$" ] && continue
    cmd=$(tr "\000" " " < "/proc/${pid}/cmdline")
    case "${cmd}" in
      *hermes*) found=1 ;;
    esac
    if ! process_env=$(tr "\000" "\n" < "/proc/${pid}/environ"); then
      echo "Cannot read environment for uid 10000 process ${pid}" >&2
      exit 1
    fi
    case "${process_env}" in
      *NEBIUS_API_KEY=*)
      echo "NEBIUS_API_KEY reached uid 10000 process ${pid}" >&2
      exit 1
      ;;
    esac
  done
  [ "${found}" = "1" ]
'

# The same launcher Hermes runs under must refuse every socket but Unix sockets and TCP streams.
socket_check="$(MSYS_NO_PATHCONV=1 docker exec -i --user 10000:10000 "${app}" sh -c \
  'dir="$(mktemp -d)" && exec pru-agent-jail --write-dir "${dir}" --connect-port 8787 -- python3 -' \
  < "${repo_root}/scripts/jail-socket-check.py")"
socket_check="${socket_check//$'\r'/}"
if [ "${socket_check}" != "jail-socket-check ok" ]; then
  echo "Jail socket check failed: ${socket_check}" >&2
  exit 1
fi

echo "Hermes container smoke passed: ${permissions}; ${socket_check}; ${health_json}"
