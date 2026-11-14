#!/bin/bash
# Build side of the loop, run wherever cargo is (Claude's cloud workspace).
# Output goes to target/loop: bin/ (Linux simbridge + smoke) and elfs/ (task ELFs).
#
#  - The VM is Ubuntu 22.04 (glibc 2.35); building on a newer distro would link
#    against a newer glibc, so cargo-zigbuild pins the glibc version.
#  - Task firmware needs its MCU target's core library. With the target installed
#    (rustup target add thumbv7em-none-eabihf), leave VALIDATE_CARGO_ARGS empty;
#    without it, build core from rust-src as below.
set -euo pipefail
cd "$(dirname "$0")/../.."
GLIBC=${GLIBC:-2.35}
OUT=target/loop

cargo zigbuild --release --target "x86_64-unknown-linux-gnu.$GLIBC" --bin simbridge --example smoke
mkdir -p "$OUT/bin"
T=${CARGO_TARGET_DIR:-target}/x86_64-unknown-linux-gnu/release
cp "$T/simbridge" "$T/examples/smoke" "$OUT/bin/"

if [ "${SKIP_TASKS:-}" != 1 ]; then
    RUSTC_BOOTSTRAP=1 \
    VALIDATE_CARGO_ARGS="${VALIDATE_CARGO_ARGS--Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem}" \
        python3 bench/validate.py --build-only
    mkdir -p "$OUT/elfs"
    cp bench/target/elfs/*.elf "$OUT/elfs/"
fi
ls -la "$OUT"/*
