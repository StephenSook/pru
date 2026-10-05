#!/usr/bin/env bash
# Install only the pinned OpenShell prover binary. This does not alter a gateway.
set -euo pipefail

version=0.1.2
archive=openshell-prover-x86_64-unknown-linux-musl.tar.gz
expected=e1c9db66ae459850cab43765d7ad569effe2ac0a338c902a1b895d0cc361fbb0
destination="${OPENSHELL_PROVER_INSTALL_DIR:-$HOME/.local/opt/openshell-prover-$version}"
download="${TMPDIR:-/tmp}/$archive"
url="https://github.com/NVIDIA/OpenShell/releases/download/v$version/$archive"

mkdir -p "$destination"
curl --fail --location --silent --show-error "$url" --output "$download"
actual=$(sha256sum "$download")
actual=${actual%% *}
if [[ "$actual" != "$expected" ]]; then
  echo "openshell-prover checksum mismatch: expected=$expected actual=$actual" >&2
  exit 1
fi
tar -xzf "$download" -C "$destination"
prover_path=$(find "$destination" -type f -name openshell-prover -print -quit)
if [[ -z "$prover_path" ]]; then
  echo "openshell-prover executable not found under $destination" >&2
  exit 1
fi
chmod +x "$prover_path"
"$prover_path" --version
printf '%s\n' "$prover_path"
