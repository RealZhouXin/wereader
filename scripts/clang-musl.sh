#!/bin/sh
set -eu
case "${TARGET:-}" in
    armv7-unknown-linux-musleabihf|arm-unknown-linux-musleabi)
        root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
        sysroot=${REREADER_MUSL_SYSROOT:-$root/.toolchains/$TARGET}
        if [ ! -f "$sysroot/include/bits/alltypes.h" ]; then
            echo "Missing musl C headers. Run: sh scripts/setup-musl-headers.sh $TARGET" >&2
            exit 1
        fi
        exec "${REREADER_CLANG:-clang}" --sysroot="$sysroot" -isystem "$sysroot/include" "$@"
        ;;
    *) exec "${REREADER_CLANG:-clang}" "$@" ;;
esac
