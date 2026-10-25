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
mkdir -p "$dest"
cp -R packaging/. "$dest/"
cp "target/$target/release/kindle-native" "$dest/kindle-native/bin/kindle-native"
cp README.md LICENSE "$dest/kindle-native/"
cp assets/Tuffy-LICENSE.txt "$dest/kindle-native/"
chmod 755 "$dest/kindle-native/start.sh" "$dest/kindle-native/bin/kindle-native"
printf '%s\n' "$target" > "$dest/kindle-native/BUILD_TARGET"
COPYFILE_DISABLE=1 tar -czf "dist/kindle-native-$target.tar.gz" -C "$dest" .
echo "Copy the contents of $dest to the Kindle USB root."
