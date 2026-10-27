#!/bin/sh
# Install only the musl C headers. Rust supplies the target libc and linker.
set -eu
cd "$(dirname "$0")/.."
target=${1:-armv7-unknown-linux-musleabihf}
case "$target" in
    armv7-unknown-linux-musleabihf) triple=arm-linux-musleabihf ;;
    arm-unknown-linux-musleabi) triple=arm-linux-musleabi ;;
    *) echo "Unsupported musl header target: $target" >&2; exit 1 ;;
esac
root=$(pwd)
version=1.2.5
archive="$root/.toolchains/musl-$version.tar.gz"
headers="$root/.toolchains/$target"
mkdir -p "$root/.toolchains"
if [ ! -f "$archive" ]; then
    curl -fL --retry 2 --connect-timeout 15 --max-time 120 \
        "https://musl.libc.org/releases/musl-$version.tar.gz" -o "$archive.part"
    mv "$archive.part" "$archive"
fi
printf '%s  %s\n' a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4 "$archive" | sha256sum -c -
if [ ! -f "$headers/include/bits/alltypes.h" ]; then
    mkdir -p "$headers/source"
    tar -xzf "$archive" --strip-components=1 -C "$headers/source"
    (cd "$headers/source" && ./configure --target="$triple" --prefix="$headers" \
        CC="${REREADER_CLANG:-clang}" CFLAGS="--target=$triple" > configure.log 2>&1 && \
        make -s install-headers)
fi
tar -xOf "$archive" "musl-$version/COPYRIGHT" > "$root/.toolchains/musl-COPYRIGHT"
echo "musl $version headers ready: $headers/include"
