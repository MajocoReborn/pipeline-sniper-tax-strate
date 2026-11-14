#!/usr/bin/env bash
# Builds the program with cargo-build-sbf 4.6.0 and checks its sha256.
#
# The build is reproducible wherever the repository is on disk: `sniper-tax-math` is a
# member of the `programs/` workspace, so Cargo identifies it by its path inside that
# workspace. (Until 14 November it lay outside, Cargo used its absolute path, and the
# binary changed with the directory; see docs/HANDOFF.md, pitfall 1.)
#
#   scripts/build-program.sh           # build, and fail unless the hash is EXPECTED_SHA256
#   scripts/build-program.sh --any     # build, and only print the hash (for program changes)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# The current source: the 30% cap and the launch block (14 November 2026), 375,712 bytes.
EXPECTED_SHA256="db635b29f127a8b249e3b4a83693482abec03cf171a4b3783da0c3cec818e24f"

# The toolchain is part of the result: another cargo-build-sbf brings other platform tools,
# and with them another binary. The Solana CLI installer ships its own cargo-build-sbf, which
# can come first on PATH.
REQUIRED_SBF="cargo-build-sbf 4.6.0"
REQUIRED_TOOLS="platform-tools: v1.58"
version=$(cargo build-sbf --version 2>/dev/null || true)
if [[ "$version" != *"$REQUIRED_SBF"* || "$version" != *"$REQUIRED_TOOLS"* ]]; then
  echo "Needs $REQUIRED_SBF with $REQUIRED_TOOLS; found:" >&2
  echo "${version:-no cargo-build-sbf}" >&2
  echo "Install it with: cargo install cargo-build-sbf --version 4.6.0 --locked" >&2
  echo "and make sure ~/.cargo/bin comes first on PATH (which -a cargo-build-sbf)." >&2
  exit 1
fi

(cd programs && cargo build-sbf)

actual=$(sha256sum programs/target/deploy/sniper_tax.so | cut -d' ' -f1)
echo "programs/target/deploy/sniper_tax.so: $actual ($(stat -c %s programs/target/deploy/sniper_tax.so) bytes)" >&2
if [[ "${1:-}" != "--any" && "$actual" != "$EXPECTED_SHA256" ]]; then
  echo "Expected $EXPECTED_SHA256." >&2
  echo "If you changed the program on purpose, rerun with --any and update EXPECTED_SHA256." >&2
  exit 1
fi
