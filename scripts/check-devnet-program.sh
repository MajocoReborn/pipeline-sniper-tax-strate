#!/usr/bin/env bash
# Checks that devnet runs the version pinned in DEVNET_SHA256, and says whether that is the
# binary in programs/target/deploy/sniper_tax.so (build it with scripts/build-program.sh).
# Reads only; needs devnet RPC. After deploying a new version, update DEVNET_SHA256.
#
# The ProgramData account can be larger than the binary (after an extend), with zeros at the
# end; those are cut off before hashing, as `solana-verify get-program-hash` does.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RPC_URL="${RPC_URL:-https://api.devnet.solana.com}"
PROGRAM_ID="EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b"
SO=programs/target/deploy/sniper_tax.so
# The version on devnet since slot 522277672 (14 November 2026, B14), 375,712 bytes.
DEVNET_SHA256="db635b29f127a8b249e3b4a83693482abec03cf171a4b3783da0c3cec818e24f"
DEVNET_SIZE=375712

rpc() {
  curl -sS --fail -m 60 -X POST "$RPC_URL" -H 'Content-Type: application/json' -d "$1"
}

program=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$PROGRAM_ID\",{\"encoding\":\"base64\"}]}")
program_data=$(printf '%s' "$program" | python3 -I -c '
import base64, json, sys
value = json.load(sys.stdin)["result"]["value"]
assert value is not None, "no program at this address"
assert value["owner"] == "BPFLoaderUpgradeab1e11111111111111111111111", "not an upgradeable program"
assert value["executable"], "not executable"
raw = base64.b64decode(value["data"][0])
assert raw[:4] == b"\x02\x00\x00\x00", "not UpgradeableLoaderState::Program"
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = int.from_bytes(raw[4:36], "big")
out = ""
while n:
    n, r = divmod(n, 58)
    out = alphabet[r] + out
print("1" * (len(raw[4:36]) - len(raw[4:36].lstrip(b"\0"))) + out)
')

data=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$program_data\",{\"encoding\":\"base64\"}]}")
printf '%s' "$data" | python3 -I -c '
import base64, hashlib, json, os, struct, sys
so_path, program_data, expected, size = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
raw = base64.b64decode(json.load(sys.stdin)["result"]["value"]["data"][0])
tag, slot = struct.unpack_from("<IQ", raw, 0)
assert tag == 3, "not UpgradeableLoaderState::ProgramData"
elf = raw[45:]
onchain = elf[:size]
remote = hashlib.sha256(onchain).hexdigest()
if any(elf[size:]):
    remote = "a longer binary than the pinned version"
print(f"program data   {program_data}, last deployed at slot {slot}")
print(f"on devnet      {remote}")
print(f"pinned         {expected}")
if os.path.exists(so_path):
    local = hashlib.sha256(open(so_path, "rb").read()).hexdigest()
    same = "the same as devnet" if local == remote else "not deployed yet"
    print(f"local build    {local} ({same})")
sys.exit(0 if remote == expected else 1)
' "$SO" "$program_data" "$DEVNET_SHA256" "$DEVNET_SIZE" || { echo "Devnet does not run the pinned version." >&2; exit 1; }
echo "Devnet runs the pinned version."
