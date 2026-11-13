#!/usr/bin/env bash
# Downloads the Pipeline router program that is live on devnet into fixtures/pipeline.so,
# for the LiteSVM integration tests. Nothing is built: this is the exact binary devnet runs.
#
# The router is still upgradeable, so the expected version is pinned below. If Pipeline
# deploys a new router, this script stops; check the change, then update the pin with
# `scripts/fetch-pipeline-router.sh --accept-new`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RPC_URL="${RPC_URL:-https://api.devnet.solana.com}"
# ProgramData account of PiPEf4fFBpYXCtgfiWgVWzZNXnq2wZ7bnXidJCTwf34 on devnet.
PROGRAM_DATA="3dpKiLEDnLxCDBybnc5N9iVSLDWpvVBhE4971w59JETL"
# Pinned version: deployed at devnet slot 520510501 (a reproducible rebuild of the same router source as slot 520302321).
EXPECTED_SHA256="57a93bbaef9d2156791567bb50fc253e2bda0c295c73b195ddd93e5406d9be6c"

mkdir -p fixtures
response=$(curl -sS --fail -m 60 -X POST "$RPC_URL" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$PROGRAM_DATA\",{\"encoding\":\"base64\"}]}")

actual=$(printf '%s' "$response" | python3 -I -c '
import base64, hashlib, json, struct, sys
value = json.load(sys.stdin)["result"]["value"]
assert value["owner"] == "BPFLoaderUpgradeab1e11111111111111111111111", "not a program-data account"
raw = base64.b64decode(value["data"][0])
tag, slot = struct.unpack_from("<IQ", raw, 0)
assert tag == 3, "not UpgradeableLoaderState::ProgramData"
elf = raw[45:]  # 4-byte tag, 8-byte slot, 1 + 32 bytes for the upgrade authority
assert elf[:4] == b"\x7fELF", "program data is not an ELF"
open("fixtures/pipeline.so", "wb").write(elf)
print(f"deployed at slot {slot}", file=sys.stderr)
print(hashlib.sha256(elf).hexdigest())
')

if [[ "$actual" != "$EXPECTED_SHA256" ]]; then
  if [[ "${1:-}" == "--accept-new" ]]; then
    sed -i "s/^EXPECTED_SHA256=.*/EXPECTED_SHA256=\"$actual\"/" "$0"
    echo "Pinned the new router version: $actual" >&2
  else
    echo "The devnet router changed (sha256 $actual, expected $EXPECTED_SHA256)." >&2
    echo "Check what changed, then rerun with --accept-new." >&2
    exit 1
  fi
fi
echo "fixtures/pipeline.so: $actual" >&2
