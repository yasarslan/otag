#!/usr/bin/env bash
#
# Build a release disk image containing Otag.app and an Applications shortcut.
#
# Usage: scripts/make-dmg.sh

set -euo pipefail

if [ "$(uname -s)" != Darwin ]; then
    echo "Otag disk images can only be built on macOS." >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

for tool in hdiutil codesign shasum; do
    if ! command -v "$tool" > /dev/null 2>&1; then
        echo "$tool is required to build and verify an Otag disk image." >&2
        exit 1
    fi
done

version="$(sed -n '/^version = "/ { s/^version = "\([^"]*\)"$/\1/p; q; }' Cargo.toml)"
: "${version:?cannot read the version from Cargo.toml}"
arch="$(uname -m)"
filename="Otag-$version-$arch.dmg"
dmg="target/$filename"

mkdir -p target
work_dir="$(mktemp -d target/.otag-dmg.XXXXXX)"
trap 'rm -rf "$work_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

staging="$work_dir/staging"
mkdir "$staging"
scripts/bundle-app.sh --output "$staging"
ln -s /Applications "$staging/Applications"

echo "==> creating $dmg"
hdiutil create -srcfolder "$staging" -volname Otag -format UDZO "$work_dir/$filename"
echo "==> verifying the disk image and app signature"
hdiutil verify "$work_dir/$filename"
codesign --verify --deep --strict "$staging/Otag.app"

(
    cd "$work_dir"
    shasum -a 256 "$filename" > "$filename.sha256"
)

# Publish only a verified image. Serialize replacement of the image and checksum.
lock="$dmg.lock"
if ! mkdir "$lock"; then
    echo "Cannot publish $dmg while its output lock exists: $lock" >&2
    exit 1
fi
trap 'rm -rf "$work_dir"; rmdir "$lock"' EXIT
rm -f "$dmg.sha256"
mv -f "$work_dir/$filename" "$dmg"
mv -f "$work_dir/$filename.sha256" "$dmg.sha256"

size="$(stat -f %z "$dmg")"
read -r checksum _ < "$dmg.sha256"
rm -rf "$staging"
echo
echo "built $dmg ($size bytes)"
echo "SHA-256: $checksum"
