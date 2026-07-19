#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source_dir="$repo_root/native/quip-ime"
stage_bundle="$repo_root/target/quip-native-ime/QuipNativeIME.bundle-stage"
iconset_dir="$repo_root/target/quip-native-ime/Quip.iconset"
client_shim_object="$repo_root/target/quip-native-ime/client_shim.o"

rm -rf "$stage_bundle"
rm -rf "$iconset_dir"
mkdir -p "$stage_bundle/Contents/MacOS" "$stage_bundle/Contents/Resources/en.lproj"
mkdir -p "$iconset_dir"
cp "$source_dir/Info.plist" "$stage_bundle/Contents/Info.plist"
cp "$source_dir/en.lproj/InfoPlist.strings" \
  "$stage_bundle/Contents/Resources/en.lproj/InfoPlist.strings"
sips -z 32 32 "$repo_root/src-tauri/icons/icon.png" \
  --out "$iconset_dir/icon_32x32.png" >/dev/null
sips -z 128 128 "$repo_root/src-tauri/icons/icon.png" \
  --out "$iconset_dir/icon_128x128.png" >/dev/null
cp "$repo_root/src-tauri/icons/icon.png" "$iconset_dir/icon_512x512.png"
iconutil -c icns -o "$stage_bundle/Contents/Resources/Quip.icns" "$iconset_dir"

clang \
  -fobjc-arc \
  -O2 \
  -framework AppKit \
  -c "$source_dir/client_shim.m" \
  -o "$client_shim_object"

swiftc \
  -O \
  -module-name QuipNativeIME \
  -import-objc-header "$source_dir/client_shim.h" \
  -framework Cocoa \
  -framework InputMethodKit \
  -framework Network \
  "$source_dir/server.swift" \
  "$source_dir/engine_bridge.swift" \
  "$source_dir/controller.swift" \
  "$client_shim_object" \
  -o "$stage_bundle/Contents/MacOS/QuipNativeIME"

plutil -lint "$stage_bundle/Contents/Info.plist"
echo "Built native input method stage at $stage_bundle"
