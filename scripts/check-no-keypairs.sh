#!/usr/bin/env bash
# Fails if a tracked or staged JSON file looks like a Solana keypair (a JSON array of 64
# byte values, as solana-keygen writes them). Run by the pre-commit hook and by CI.
# Modeled on Pipeline's scripts/check-no-keypairs.sh (MetalLegBob/pipeline, Apache-2.0).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

found=0
while IFS= read -r file; do
  [[ -f "$file" ]] || continue
  if python3 - "$file" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(1)
looks_like_key = (
    isinstance(data, list)
    and len(data) == 64
    and all(isinstance(b, int) and 0 <= b <= 255 for b in data)
)
sys.exit(0 if looks_like_key else 1)
PY
  then
    echo "Refusing: $file looks like a Solana keypair. Keep keys outside the repository." >&2
    found=1
  fi
done < <({ git ls-files; git diff --cached --name-only --diff-filter=ACMR; } | sort -u | grep -E '\.json$' || true)
exit "$found"
