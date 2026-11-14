#!/bin/bash
# One-time (per VM session) setup: Renode, plus the prebuilt Linux binaries
# from target/loop/bin copied out of the mount so they can be executed.
set -euo pipefail
. "$(dirname "$0")/env.sh"

if [ ! -x "$HOME/renode/renode" ]; then
    url="https://github.com/renode/renode/releases/download/v$RENODE_VERSION/renode-$RENODE_VERSION.linux-portable.tar.gz"
    curl -sSL -o "$HOME/renode.tgz" "$url"
    tar xzf "$HOME/renode.tgz" -C "$HOME" && rm "$HOME/renode.tgz"
    ln -sfn "$HOME/renode_$RENODE_VERSION-portable" "$HOME/renode"
fi
"$SIMBRIDGE_RENODE" --version | head -1

mkdir -p "$HOME/bin"
cp "$SB_ROOT/target/loop/bin/"* "$HOME/bin/"
chmod +x "$HOME/bin/"*
ls -la "$HOME/bin"
