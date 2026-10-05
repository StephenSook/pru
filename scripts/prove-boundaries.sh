#!/usr/bin/env bash
# Exact spike command: openshell-prover check CANDIDATE --boundary BOUNDARY.
# Exit 0 means within, 1 means exceeds, 2 means input error, and 3 means
# unsupported or inconclusive. Exit 3 always fails this script.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
default_prover="$HOME/.local/opt/openshell-prover-0.1.2/openshell-prover"
prover="${OPENSHELL_PROVER:-$default_prover}"
if [[ ! -x "$prover" ]]; then
  bash "$repo/scripts/install-openshell-prover.sh"
fi
"$prover" --version

found=0
for boundary in "$repo"/eval/boundaries/*.yaml; do
  if [[ ! -f "$boundary" ]]; then
    continue
  fi
  found=$((found + 1))
  echo "case=compiled_boundary_self file=$(basename "$boundary")"
  set +e
  "$prover" check "$boundary" --boundary "$boundary"
  rc=$?
  set -e
  echo "exit=$rc"
  if [[ $rc -eq 3 ]]; then
    echo "unsupported or inconclusive boundary proof" >&2
    exit 3
  fi
  if [[ $rc -ne 0 ]]; then
    echo "compiled boundary did not prove within itself" >&2
    exit "$rc"
  fi
done
if [[ $found -eq 0 ]]; then
  echo "no compiled boundaries found" >&2
  exit 2
fi

boundary="$repo/eval/boundaries/synthetic-client.yaml"
candidate=$(mktemp)
trap 'rm -f "$candidate"' EXIT
cp "$boundary" "$candidate"
cat >> "$candidate" <<'YAML'
  planted_exfiltration:
    name: planted_exfiltration
    endpoints:
      - host: example.com
        port: 443
        protocol: rest
        enforcement: enforce
        rules:
          - allow:
              method: POST
              path: /**
    binaries:
      - path: /opt/hermes/.venv/bin/python
YAML

echo "case=planted_extra_host expected=exceeds_boundary"
set +e
"$prover" check "$candidate" --boundary "$boundary"
rc=$?
set -e
echo "exit=$rc"
if [[ $rc -eq 3 ]]; then
  echo "unsupported or inconclusive planted proof" >&2
  exit 3
fi
if [[ $rc -ne 1 ]]; then
  echo "planted extra host did not produce exceeds_boundary" >&2
  exit 1
fi
