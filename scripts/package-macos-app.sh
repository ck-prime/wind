#!/bin/sh
# Assemble a background Wind.app from the release supervisor binary.
# The script does not notarize, install a LaunchAgent, or request admin rights.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
output=${1:-"$HOME/Desktop/Wind.app"}
case "$output" in
/*) ;;
*)
    echo "output path must be absolute: $output" >&2
    exit 2
    ;;
esac

display_name=$(awk -F'"' '/^display_name = / { print $2; exit }' "$root/config/product.toml")
identifier=$(awk -F'"' '/^identifier = / { print $2; exit }' "$root/config/product.toml")
version=$(awk -F'"' '/^version = / { print $2; exit }' "$root/config/product.toml")
if [ -z "$display_name" ] || [ -z "$identifier" ] || [ -z "$version" ]; then
    echo "config/product.toml is missing display_name, identifier, or version" >&2
    exit 1
fi

cargo build --release --manifest-path "$root/Cargo.toml" -p wind-supervisor

rm -rf "$output"
mkdir -p "$output/Contents/MacOS" "$output/Contents/Resources"
cp "$root/target/release/wind" "$output/Contents/MacOS/Wind"
chmod 755 "$output/Contents/MacOS/Wind"
cp "$root/assets/branding/logo.svg" "$output/Contents/Resources/logo.svg"

cat > "$output/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>Wind</string>
	<key>CFBundleIconFile</key>
	<string></string>
	<key>CFBundleIdentifier</key>
	<string>${identifier}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>${display_name}</string>
	<key>CFBundleDisplayName</key>
	<string>${display_name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.3</string>
	<key>LSBackgroundOnly</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSScreenCaptureUsageDescription</key>
	<string>Wind captures the display when a tracking interval closes.</string>
	<key>NSAccessibilityUsageDescription</key>
	<string>Wind reads the foreground browser address field.</string>
</dict>
</plist>
EOF

# Ad-hoc signature only. This is not a Developer ID signature and not notarization.
codesign --force --sign - "$output/Contents/MacOS/Wind"
codesign --force --sign - "$output"

echo "$output"
