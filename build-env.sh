#!/usr/bin/env bash
# Defaults work with native system libraries; optional sysroots stay outside the checkout.
export OPSPIER_CACHE_DIR="${OPSPIER_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/opspier}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$OPSPIER_CACHE_DIR/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
if [[ -n "${OPSPIER_SYSROOT:-}" ]]; then
  export PKG_CONFIG_SYSROOT_DIR="$OPSPIER_SYSROOT"
  export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-$OPSPIER_SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$OPSPIER_SYSROOT/usr/share/pkgconfig}"
  export LIBRARY_PATH="$OPSPIER_SYSROOT/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
fi
