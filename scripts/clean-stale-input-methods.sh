#!/bin/sh
set -eu

timestamp=$(date +%Y%m%d-%H%M%S)
preferences_backup="/tmp/com.apple.HIToolbox-before-quip-clean-${timestamp}.plist"
working_preferences=$(mktemp /tmp/quip-hitoolbox.XXXXXX)
lsregister='/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'

cleanup_working_preferences() {
  rm -f "$working_preferences"
}
trap cleanup_working_preferences EXIT HUP INT TERM

defaults export com.apple.HIToolbox "$working_preferences"
cp "$working_preferences" "$preferences_backup"

for array_name in AppleEnabledInputSources AppleSelectedInputSources AppleInputSourceHistory; do
  for index in $(jot 65 64 0); do
    bundle_id=$(/usr/libexec/PlistBuddy \
      -c "Print :${array_name}:${index}:'Bundle ID'" \
      "$working_preferences" 2>/dev/null || true)
    case "$bundle_id" in
      com.hackthe6ix.quip*|com.hackthe6ix.inputmethod.Quip)
        /usr/libexec/PlistBuddy \
          -c "Delete :${array_name}:${index}" \
          "$working_preferences"
        ;;
    esac
  done
done

defaults import com.apple.HIToolbox "$working_preferences"

for stale_bundle in \
  "${HOME}/Library/Input Methods/Quip.app" \
  "${HOME}/Library/Input Methods/Quip Input.app" \
  "${HOME}/Library/Input Methods/Quip Native.app"
do
  "$lsregister" -u "$stale_bundle" 2>/dev/null || true
  if [ -e "$stale_bundle" ]; then
    stale_name=$(basename "$stale_bundle" .app)
    backup_bundle="/tmp/${stale_name}-stale-${timestamp}.app.disabled"
    if [ -e "$backup_bundle" ]; then
      echo "Refusing to overwrite backup at $backup_bundle" >&2
      exit 1
    fi
    mv "$stale_bundle" "$backup_bundle"
    echo "Moved stale user bundle to $backup_bundle"
  fi
done

"$lsregister" -u '/Library/Input Methods/Quip.app' 2>/dev/null || true
/usr/bin/pkill -f '/Input Methods/Quip.*\.app/Contents/MacOS/' 2>/dev/null || true
killall cfprefsd 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true

echo "Removed Quip records from enabled, selected, and history preferences."
echo "Preferences backup: $preferences_backup"
if [ -e '/Library/Input Methods/Quip.app' ]; then
  echo "The root-owned /Library/Input Methods/Quip.app remains on disk but is unregistered."
fi
