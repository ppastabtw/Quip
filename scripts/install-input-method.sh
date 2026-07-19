#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage_bundle="$repo_root/target/quip-native-ime/QuipNativeIME.bundle-stage"
installed_bundle='/Library/Input Methods/Quip Native.app'
user_bundle="${HOME}/Library/Input Methods/Quip Native.app"
privileged_stage_dir=$(mktemp -d "${TMPDIR:-/tmp}/quip-native-install.XXXXXX")
privileged_stage="$privileged_stage_dir/Quip Native.app"
timestamp=$(date +%Y%m%d-%H%M%S)
old_system_backup="/tmp/Quip-system-stale-${timestamp}.app.disabled"
native_system_backup="/tmp/Quip-Native-system-stale-${timestamp}.app.disabled"
native_user_backup="/tmp/Quip-Native-user-stale-${timestamp}.app.disabled"
lsregister='/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'

cleanup_privileged_stage() {
  rm -rf -- "$privileged_stage_dir"
}
trap cleanup_privileged_stage EXIT HUP INT TERM

"$repo_root/scripts/build-native-input-method.sh"

signing_identity=${QUIP_CODESIGN_IDENTITY:--}
if [ "$signing_identity" = "auto" ]; then
  signing_identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F '"' '/Apple Development:|Developer ID Application:/ { print $2; exit }')
fi
if [ "$signing_identity" = "-" ]; then
  codesign --force --deep --sign - "$stage_bundle"
  echo "Signed Quip Native with an ad-hoc local signature"
elif [ -n "$signing_identity" ]; then
  if codesign --force --deep --sign "$signing_identity" "$stage_bundle"; then
    echo "Signed Quip Native with $signing_identity"
  else
    echo "Development identity signing failed; falling back to an ad-hoc signature" >&2
    codesign --force --deep --sign - "$stage_bundle"
  fi
else
  codesign --force --deep --sign - "$stage_bundle"
  echo "No development identity found; used an ad-hoc signature"
fi
codesign --verify --deep --strict --verbose=2 "$stage_bundle"

# A privileged shell does not inherit the invoking app's macOS privacy access
# to Documents/Desktop. Copy the signed bundle to /tmp as the current user so
# the administrator process can read it without Full Disk Access.
/usr/bin/ditto "$stage_bundle" "$privileged_stage"
codesign --verify --deep --strict --verbose=2 "$privileged_stage"

admin_command="/bin/mkdir -p '/Library/Input Methods'; if [ -e '/Library/Input Methods/Quip.app' ]; then /bin/mv '/Library/Input Methods/Quip.app' '$old_system_backup'; fi; if [ -e '$installed_bundle' ]; then /bin/mv '$installed_bundle' '$native_system_backup'; fi; /usr/bin/ditto '$privileged_stage' '$installed_bundle'; /usr/sbin/chown -R root:wheel '$installed_bundle'"
QUIP_ADMIN_COMMAND="$admin_command" osascript -l JavaScript -e '
  ObjC.import("Foundation");
  const app = Application.currentApplication();
  app.includeStandardAdditions = true;
  const command = ObjC.unwrap(
    $.NSProcessInfo.processInfo.environment.objectForKey("QUIP_ADMIN_COMMAND")
  );
  app.doShellScript(command, { administratorPrivileges: true });
'

codesign --verify --deep --strict --verbose=2 "$installed_bundle"
"$lsregister" -u '/Library/Input Methods/Quip.app' 2>/dev/null || true
"$lsregister" -u "$user_bundle" 2>/dev/null || true
if [ -e "$user_bundle" ]; then
  if [ -e "$native_user_backup" ]; then
    echo "Refusing to overwrite backup at $native_user_backup" >&2
    exit 1
  fi
  mv "$user_bundle" "$native_user_backup"
  echo "Moved the ignored user-level bundle to $native_user_backup"
fi

QUIP_INPUT_METHOD_BUNDLE="$installed_bundle" osascript -l JavaScript -e '
  ObjC.import("Carbon");
  ObjC.import("Foundation");
  const path = ObjC.unwrap(
    $.NSProcessInfo.processInfo.environment.objectForKey("QUIP_INPUT_METHOD_BUNDLE")
  );
  const status = $.TISRegisterInputSource($.NSURL.fileURLWithPathIsDirectory(path, true));
  if (status !== 0) {
    throw new Error("TISRegisterInputSource failed with status " + status);
  }
'
"$lsregister" -f "$installed_bundle"
killall QuipNativeIME 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true
xcrun swift "$repo_root/scripts/select-native-input-method.swift"

echo "Installed the standalone input method at $installed_bundle"
echo "Old system bundle backup: $old_system_backup"
echo "Log file: ${HOME}/Library/Logs/QuipNativeIME.log"
echo "Add Quip Native in Keyboard Settings, then select it from the input menu."
