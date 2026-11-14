#!/bin/bash
# Run the prebuilt smoke test against the prebuilt server. Args select boards.
# All 7 boards take ~3 min, longer than one shell call: run it with nohup.
set -euo pipefail
. "$(dirname "$0")/env.sh"
cp "$SB_ROOT/target/loop/bin/"* "$HOME/bin/" && chmod +x "$HOME/bin/"*
rm -rf "$SIMBRIDGE_WORKDIR"
cd "$HOME" && exec "$HOME/bin/smoke" "$@"
