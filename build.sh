#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
. ./build-env.sh
npm test
npm run build
cd src-tauri
cargo fmt --check
cargo test --locked
cargo build --release --locked
