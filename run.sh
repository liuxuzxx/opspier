#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")"
. ./build-env.sh
if [[ -n "${PKG_CONFIG_SYSROOT_DIR:-}" ]]; then
  export LD_LIBRARY_PATH="$PKG_CONFIG_SYSROOT_DIR/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
exec "${OPSPIER_APPLICATION:-$CARGO_TARGET_DIR/release/opspier}" "$@"
