#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target=${1:-armv7-unknown-linux-musleabihf}
case "$target" in
    armv7-unknown-linux-musleabihf|arm-unknown-linux-musleabi) ;;
    *) echo "Unsupported package target: $target" >&2; exit 1 ;;
esac
cargo build --locked --release --no-default-features --features kindle --target "$target"
dest="dist/$target/install"
mkdir -p "$dest/rereader/bin"
cp -R packaging/. "$dest/"
cp "target/$target/release/rereader" "$dest/rereader/bin/rereader"
cp README.md LICENSE "$dest/rereader/"
cp assets/Tuffy-LICENSE.txt "$dest/rereader/"
chmod 755 "$dest/rereader/start.sh" "$dest/rereader/bin/rereader"
printf '%s\n' "$target" > "$dest/rereader/BUILD_TARGET"
COPYFILE_DISABLE=1 tar -czf "dist/rereader-$target.tar.gz" -C "$dest" .
echo "Copy the contents of $dest to the Kindle USB root."
