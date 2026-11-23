#!/usr/bin/env bash
# Fails when crates/cv-model/src/modules/mod.rs is stale vs the lane dirs.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run -q -p xtask -- codegen --check
