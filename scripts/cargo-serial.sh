#!/usr/bin/env bash
# One build owner and one target directory keep concurrent agents from multiplying artifacts.
set -euo pipefail
repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_bin="${FMONERO_CARGO:-${HOME}/.cargo/bin/cargo}"
if [ ! -x "$cargo_bin" ]; then cargo_bin="$(command -v cargo)"; fi
lock_file="${XDG_RUNTIME_DIR:-/tmp}/fmonero-build.lock"
cd -- "$repo"
export CARGO_TARGET_DIR="$repo/target" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
exec flock -w 300 "$lock_file" "$cargo_bin" "$@"
