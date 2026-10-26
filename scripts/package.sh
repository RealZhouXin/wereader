#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target=${1:-armv7-unknown-linux-musleabihf}
case "$target" in
    armv7-unknown-linux-musleabihf|arm-unknown-linux-musleabi) ;;
    *) echo "Unsupported package target: $target" >&2; exit 1 ;;
esac
cargo build --locked --release --no-default-features --features kindle --target "$target"
# Snapshot the matching source and dependencies before publishing either archive.
mkdir -p dist
source_stage=$(mktemp -d "dist/source.XXXXXX")
trap 'rm -rf "$source_stage"' EXIT HUP INT TERM
source_dir="$source_stage/rereader-source"
mkdir -p "$source_dir/.cargo"
cp Cargo.toml Cargo.lock build.rs README.md LICENSE THIRD_PARTY_NOTICES.md SOURCE.md "$source_dir/"
cp -R src ui assets scripts packaging docs "$source_dir/"
cp .cargo/config.toml "$source_dir/.cargo/"
if [ -d vendor ]; then
    # Repackaging an extracted source archive: retain its source replacement config.
    cp -R vendor "$source_dir/"
else
    # Run inside the snapshot so Cargo emits a relocatable directory = "vendor".
    (cd "$source_dir" && cargo vendor --locked --versioned-dirs vendor > .cargo/vendor.toml)
    cat "$source_dir/.cargo/vendor.toml" >> "$source_dir/.cargo/config.toml"
    rm "$source_dir/.cargo/vendor.toml"
fi
{
    printf 'target=%s\n' "$target"
    rustc --version
    cargo --version
} > "$source_dir/BUILD_INFO"
dest="dist/$target/install"
mkdir -p "$dest/rereader/bin"
cp -R packaging/. "$dest/"
cp "target/$target/release/rereader" "$dest/rereader/bin/rereader"
cp README.md LICENSE THIRD_PARTY_NOTICES.md SOURCE.md "$dest/rereader/"
cp "$source_dir/vendor/slint-backend-kindle-0.7.1/LICENSE-MIT" "$dest/rereader/slint-backend-kindle-LICENSE-MIT"
for dependency in "$source_dir"/vendor/*; do
    for notice in "$dependency"/LICENSE* "$dependency"/LICENCE* "$dependency"/COPYING* "$dependency"/NOTICE* "$dependency"/COPYRIGHT*; do
        [ -f "$notice" ] || continue
        notice_dir="$dest/rereader/licenses/$(basename "$dependency")"
        mkdir -p "$notice_dir"
        cp "$notice" "$notice_dir/"
    done
done
cp assets/Tuffy-LICENSE.txt "$dest/rereader/"
cp assets/NotoSansSC-LICENSE.txt "$dest/rereader/"
chmod 755 "$dest/rereader/start.sh" "$dest/rereader/bin/rereader"
printf '%s\n' "$target" > "$dest/rereader/BUILD_TARGET"
COPYFILE_DISABLE=1 tar -czf "dist/rereader-$target-source.tar.gz" -C "$source_stage" rereader-source
COPYFILE_DISABLE=1 tar -czf "dist/rereader-$target.tar.gz" -C "$dest" .
echo "Copy the contents of $dest to the Kindle USB root."
echo "Publish dist/rereader-$target.tar.gz together with dist/rereader-$target-source.tar.gz."
