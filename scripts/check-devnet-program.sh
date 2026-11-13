#!/usr/bin/env bash
# Checks that the program on devnet is the binary in programs/target/deploy/sniper_tax.so,
# byte for byte. Build it first with scripts/build-program.sh. Reads only; needs devnet RPC.
#
# The ProgramData account can be larger than the binary (after an extend), with zeros at the
# end, so only the first <size of the binary> bytes are compared.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RPC_URL="${RPC_URL:-https://api.devnet.solana.com}"
PROGRAM_ID="EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b"
SO=programs/target/deploy/sniper_tax.so

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
import base64, hashlib, json, struct, sys
so = open(sys.argv[1], "rb").read()
raw = base64.b64decode(json.load(sys.stdin)["result"]["value"]["data"][0])
tag, slot = struct.unpack_from("<IQ", raw, 0)
assert tag == 3, "not UpgradeableLoaderState::ProgramData"
elf = raw[45:]
onchain = elf[: len(so)]
assert not any(elf[len(so):]), "the account holds more than this binary"
local = hashlib.sha256(so).hexdigest()
remote = hashlib.sha256(onchain).hexdigest()
print(f"program data   {sys.argv[2]}, last deployed at slot {slot}")
print(f"local binary   {local} ({len(so)} bytes)")
print(f"on devnet      {remote}")
sys.exit(0 if local == remote else 1)
' "$SO" "$program_data" || { echo "The program on devnet is not this binary." >&2; exit 1; }
echo "The program on devnet is this binary."
