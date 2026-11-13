#!/usr/bin/env bash
# Builds the program exactly as deployed on devnet, and checks its sha256.
#
# The binary depends on where the repository is on disk. `programs/` is its own Cargo
# workspace, and `crates/sniper-tax-math` lies outside it, so Cargo identifies that crate by
# its absolute path. The path feeds the hash in every symbol name, the symbols are ordered by
# name, and so the layout of the binary changes with the path (the size does not). The
# version on devnet was built with the repository at /home/user/Pipeline-Sniper-Tax.
#
# This script therefore copies the tracked files (with any local edits) to that path, builds
# there with cargo-build-sbf 4.6.0, and copies the result back to programs/target/deploy/.
# Creating /home/user may need sudo once: `sudo mkdir -p /home/user && sudo chown "$USER" /home/user`.
#
#   scripts/build-program.sh           # build, and fail unless the hash is EXPECTED_SHA256
#   scripts/build-program.sh --any     # build, and only print the hash (for program changes)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

BUILD_ROOT="${BUILD_ROOT:-/home/user/Pipeline-Sniper-Tax}"
# The version on devnet since slot 520838510 (10 November 2026), 375,568 bytes.
EXPECTED_SHA256="9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd"
MARKER=".sniper-tax-build-root"

repo="$(pwd -P)"
if [[ "$repo" == "$(cd "$BUILD_ROOT" 2>/dev/null && pwd -P)" ]]; then
  echo "The repository itself is at $BUILD_ROOT; building in place." >&2
  (cd programs && cargo build-sbf)
else
  if [[ -e "$BUILD_ROOT" && ! -e "$BUILD_ROOT/$MARKER" && -n "$(ls -A "$BUILD_ROOT")" ]]; then
    echo "Refusing: $BUILD_ROOT exists and was not made by this script." >&2
    exit 1
  fi
  mkdir -p "$BUILD_ROOT"
  touch "$BUILD_ROOT/$MARKER"
  # Only what the program build reads, so stale files can't leak in.
  rm -rf "$BUILD_ROOT/programs/sniper-tax" "$BUILD_ROOT/crates"
  git ls-files -co --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml programs crates \
    | tar -cf - -T - | tar -xf - -C "$BUILD_ROOT"
  (cd "$BUILD_ROOT/programs" && cargo build-sbf)
  mkdir -p programs/target/deploy
  cp "$BUILD_ROOT/programs/target/deploy/sniper_tax.so" programs/target/deploy/sniper_tax.so
fi

actual=$(sha256sum programs/target/deploy/sniper_tax.so | cut -d' ' -f1)
echo "programs/target/deploy/sniper_tax.so: $actual ($(stat -c %s programs/target/deploy/sniper_tax.so) bytes)" >&2
if [[ "${1:-}" != "--any" && "$actual" != "$EXPECTED_SHA256" ]]; then
  echo "This is not the version on devnet ($EXPECTED_SHA256)." >&2
  echo "If you changed the program on purpose, rerun with --any, and update EXPECTED_SHA256 once it is deployed." >&2
  exit 1
fi
