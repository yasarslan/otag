#!/usr/bin/env bash
#
# Build Otag.app: the daemon, the app, and a bundle holding both.
#
# The app runs the daemon that ships inside it, so that the two always match.
# Signing is ad-hoc, which is enough to run the app on the machine that built
# it; a release build with a real identity is a separate job.
#
# Usage: scripts/bundle-app.sh [--debug] [--output DIR]

set -euo pipefail

configuration="release"
output_dir="target"

while [ $# -gt 0 ]; do
    case "$1" in
        --debug)
            configuration="debug"
            shift
            ;;
        --output)
            output_dir="${2:?--output needs a directory}"
            shift 2
            ;;
        -h | --help)
            sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

for tool in cargo swift; do
    if ! command -v "$tool" > /dev/null 2>&1; then
        echo "$tool is not installed, so the app cannot be built." >&2
        exit 1
    fi
done

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
: "${version:?cannot read the version from Cargo.toml}"

echo "==> building the daemon ($configuration)"
if [ "$configuration" = "release" ]; then
    cargo build --release --locked --bin otag
    daemon_binary="target/release/otag"
else
    cargo build --locked --bin otag
    daemon_binary="target/debug/otag"
fi

echo "==> building the app ($configuration)"
swift build --package-path app --configuration "$configuration" --product Otag
app_binary="$(swift build --package-path app --configuration "$configuration" --show-bin-path)/Otag"

bundle="$output_dir/Otag.app"
echo "==> assembling $bundle"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"

cp "$app_binary" "$bundle/Contents/MacOS/Otag"
# The daemon travels with the app: a bundle must never talk to a daemon of a
# different version than the protocol it was built against.
cp "$daemon_binary" "$bundle/Contents/Resources/otag"

# SwiftPM puts a target's resources in a bundle of their own beside the
# binary, and `Bundle.module` looks for it next to the executable. Without
# this the app ships with no strings at all and every label in the window
# shows its own key.
bin_path="$(swift build --package-path app --configuration "$configuration" --show-bin-path)"
for resources in "$bin_path"/*.bundle; do
    [ -e "$resources" ] || continue
    echo "==> copying $(basename "$resources")"
    cp -R "$resources" "$bundle/Contents/MacOS/"
done

echo "==> drawing the icon"
if swift "$root/scripts/make-icon.swift" "$bundle/Contents/Resources/AppIcon.icns" \
    > /dev/null 2>&1; then
    icon_entry='    <key>CFBundleIconFile</key>
    <string>AppIcon</string>'
else
    # An icon is not worth failing a build for; the app runs without one.
    echo "    (could not draw the icon; the app will use the default one)"
    icon_entry=""
fi

cat > "$bundle/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Otag</string>
    <key>CFBundleDisplayName</key>
    <string>Otag</string>
    <key>CFBundleIdentifier</key>
    <string>dev.otag.Otag</string>
    <key>CFBundleExecutable</key>
    <string>Otag</string>
$icon_entry
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticTermination</key>
    <false/>
    <key>NSSupportsSuddenTermination</key>
    <false/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

printf 'APPL????' > "$bundle/Contents/PkgInfo"

echo "==> signing (ad-hoc)"
# Nested bundles are signed first: signing the app seals what is inside it,
# so an unsigned resource bundle in there makes the outer signature invalid.
for nested in "$bundle"/Contents/MacOS/*.bundle; do
    [ -e "$nested" ] || continue
    codesign --force --sign - --timestamp=none "$nested" > /dev/null 2>&1
done
codesign --force --sign - --timestamp=none "$bundle" > /dev/null

echo
echo "built $bundle (otag $version)"
echo "run it with: open $bundle"
