#!/usr/bin/env bash
# Install the exact solver version required by cedar-policy-symcc 0.7.0.
set -euo pipefail

version=1.3.1
archive=cvc5-Linux-x86_64-static.zip
expected=1a1cda20d2df4938fa4944a69f33ddc9172e319ece0eed0aa09c4d7abede3ed1
destination="${CVC5_INSTALL_DIR:-$HOME/.local/opt/cvc5-$version}"
download="${TMPDIR:-/tmp}/$archive"
url="https://github.com/cvc5/cvc5/releases/download/cvc5-$version/$archive"

mkdir -p "$destination"
curl --fail --location --silent --show-error "$url" --output "$download"
actual=$(sha256sum "$download")
actual=${actual%% *}
if [[ "$actual" != "$expected" ]]; then
  echo "cvc5 checksum mismatch: expected=$expected actual=$actual" >&2
  exit 1
fi
unzip -q -o "$download" -d "$destination"
cvc5_path=$(find "$destination" -type f -name cvc5 -print -quit)
if [[ -z "$cvc5_path" ]]; then
  echo "cvc5 executable not found under $destination" >&2
  exit 1
fi
chmod +x "$cvc5_path"
"$cvc5_path" --version
printf '%s\n' "$cvc5_path"
