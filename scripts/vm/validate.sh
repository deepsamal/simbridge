#!/bin/bash
# Grade the task ELFs prebuilt by `bench/validate.py --build-only` (copied to
# target/loop/elfs). The VM has no Rust toolchain, so it never builds.
set -euo pipefail
. "$(dirname "$0")/env.sh"
cp "$SB_ROOT/target/loop/bin/"* "$HOME/bin/" && chmod +x "$HOME/bin/"*
cd "$SB_ROOT" && exec python3 bench/validate.py --prebuilt target/loop/elfs "$@"
