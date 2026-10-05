#!/usr/bin/env bash
set -euo pipefail

: "${PRU_CONSENT_TOKEN:?set PRU_CONSENT_TOKEN to a minted disclosure-consent Biscuit}"
: "${PRU_CLIENT_ID:=synthetic-live-client}"
: "${PRU_GATEWAY_URL:=http://127.0.0.1:8787}"

if python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
  python_cmd=python3
elif python -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
  python_cmd=python
else
  echo "Python 3.9 or newer is required" >&2
  exit 2
fi

response_file="$(mktemp)"
trap 'rm -f "$response_file"' EXIT

started="$("$python_cmd" -c 'import time; print(time.time_ns())')"
status="$(curl --silent --show-error --output "$response_file" --write-out '%{http_code}' \
  --request POST "$PRU_GATEWAY_URL/v1/actions/llm_complete" \
  --header 'content-type: application/json' \
  --data "$("$python_cmd" - "$PRU_CONSENT_TOKEN" "$PRU_CLIENT_ID" <<'PY'
import json
import sys

print(json.dumps({
    "client_id": sys.argv[2],
    "consent_token": sys.argv[1],
    "purpose": "answer a synthetic tax-practice question",
    "arguments": {
        "messages": [{
            "role": "user",
            "content": "This is a synthetic test. Reply with the sum of 17 and 23."
        }],
        "max_tokens": 32
    }
}))
PY
)"
)"
ended="$("$python_cmd" -c 'import time; print(time.time_ns())')"

if [[ "$status" != "200" ]]; then
  echo "gateway_http_status=$status" >&2
  "$python_cmd" -m json.tool "$response_file" >&2
  exit 1
fi

"$python_cmd" - "$response_file" "$started" "$ended" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
wall_ms = (int(sys.argv[3]) - int(sys.argv[2])) / 1_000_000
print(f"decision={payload['decision']}")
print(f"route={payload['route']}")
print(f"gateway_latency_ms={payload['latency_ms']}")
print(f"wall_latency_ms={wall_ms:.1f}")
PY
