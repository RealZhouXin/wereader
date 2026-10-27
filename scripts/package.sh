#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target=armv7-unknown-linux-musleabihf
with_source=false
for arg in "$@"; do
    case "$arg" in
        --with-source) with_source=true ;;
        armv7-unknown-linux-musleabihf|arm-unknown-linux-musleabi) target=$arg ;;
        *) echo "Usage: sh scripts/package.sh [target] [--with-source]" >&2; exit 1 ;;
    esac
done
case "$target" in
    armv7-unknown-linux-musleabihf|arm-unknown-linux-musleabi) ;;
    *) echo "Unsupported package target: $target" >&2; exit 1 ;;
esac
sh scripts/setup-musl-headers.sh "$target"
cargo build --locked --release --no-default-features --features kindle --target "$target"
mkdir -p dist
if [ "$with_source" = true ]; then
    # Snapshot the matching source and dependencies before publishing either archive.
    source_stage=$(mktemp -d "dist/source.XXXXXX")
    trap 'rm -rf "$source_stage"' EXIT HUP INT TERM
    source_dir="$source_stage/rereader-source"
    mkdir -p "$source_dir/.cargo"
    cp Cargo.toml Cargo.lock build.rs README.md LICENSE THIRD_PARTY_NOTICES.md SOURCE.md "$source_dir/"
    cp -R src ui assets scripts packaging docs "$source_dir/"
    cp .cargo/config.toml "$source_dir/.cargo/"
    # Retain the exact C-header sources for offline rebuilding and their MIT notice.
    mkdir -p "$source_dir/.toolchains"
    cp .toolchains/musl-1.2.5.tar.gz "$source_dir/.toolchains/"
    cp .toolchains/musl-COPYRIGHT "$source_dir/.toolchains/"
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
fi
dest="dist/$target/install"
mkdir -p "$dest/rereader/bin"
cp -R packaging/. "$dest/"
cp "target/$target/release/rereader" "$dest/rereader/bin/rereader"
cp README.md LICENSE THIRD_PARTY_NOTICES.md SOURCE.md "$dest/rereader/"
if [ "$with_source" = true ]; then
    cp "$source_dir/vendor/slint-backend-kindle-0.7.1/LICENSE-MIT" "$dest/rereader/slint-backend-kindle-LICENSE-MIT"
    for dependency in "$source_dir"/vendor/*; do
        for notice in "$dependency"/LICENSE* "$dependency"/LICENCE* "$dependency"/COPYING* "$dependency"/NOTICE* "$dependency"/COPYRIGHT*; do
            [ -f "$notice" ] || continue
            notice_dir="$dest/rereader/licenses/$(basename "$dependency")"
            mkdir -p "$notice_dir"
            cp "$notice" "$notice_dir/"
        done
    done
fi
mkdir -p "$dest/rereader/licenses/musl-1.2.5"
cp .toolchains/musl-COPYRIGHT "$dest/rereader/licenses/musl-1.2.5/COPYRIGHT"
cp assets/Tuffy-LICENSE.txt "$dest/rereader/"
cp assets/NotoSansSC-LICENSE.txt "$dest/rereader/"
chmod 755 "$dest/rereader/start.sh" "$dest/rereader/bin/rereader"
printf '%s\n' "$target" > "$dest/rereader/BUILD_TARGET"
if [ "$with_source" = true ]; then
    COPYFILE_DISABLE=1 tar -czf "dist/rereader-$target-source.tar.gz" -C "$source_stage" rereader-source
fi
COPYFILE_DISABLE=1 tar -czf "dist/rereader-$target.tar.gz" -C "$dest" .
echo "Copy the contents of $dest to the Kindle USB root."
if [ "$with_source" = true ]; then
    echo "Publish dist/rereader-$target.tar.gz together with dist/rereader-$target-source.tar.gz."
else
    echo "Local debugging package: dist/rereader-$target.tar.gz (source packaging skipped)."
fi
