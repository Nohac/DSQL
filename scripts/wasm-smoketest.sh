#!/usr/bin/env bash
set -euo pipefail

cargo smoke-wasm-core
cargo smoke-wasm-frontend
