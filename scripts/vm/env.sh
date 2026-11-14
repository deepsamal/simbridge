# Shared settings for the Linux VM test loop. Source this; don't run it.
# The repo is the connected folder D:\agentic-embedded-rust, mounted in the VM.
export SB_ROOT="$HOME/mnt/agentic-embedded-rust/simbridge"
export RENODE_VERSION=1.17.0   # 1.16.0 ignores log-timestamps-type = Virtual
export SIMBRIDGE_RENODE="$HOME/renode/renode"
export SIMBRIDGE_SVD_DIR="$HOME/mnt/agentic-embedded-rust/svd"
export SIMBRIDGE_WORKDIR="$HOME/sbwork"   # outside the mount: Renode logs get big
export SIMBRIDGE_EXE="$HOME/bin/simbridge"
export SIMBRIDGE_ROOT="$SB_ROOT"
