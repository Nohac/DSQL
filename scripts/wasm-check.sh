#!/usr/bin/env bash
set -euo pipefail

cargo check-wasm-core
cargo check-wasm-frontend
