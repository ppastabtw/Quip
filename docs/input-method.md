# Quip InputMethodKit integration

Quip is packaged as a selectable Latin macOS input source while keeping its
existing Tauri suggestion popup. The input source and popup run in the same
process and on the same `NSApplication` event loop.

## Runtime flow

1. `QuipInputController` receives key-down events from InputMethodKit.
2. Ordinary typing returns `false`, allowing the destination to insert the
   literal key normally.
3. After 150 ms idle, Quip verifies the destination's selected UTF-16 range,
   asks the IMK client for `attributesForCharacterIndex:lineHeightRectangle:`,
   converts the AppKit screen rectangle into Tauri coordinates, and runs the
   normal prediction engine.
4. The existing non-focusable `suggestions` webview appears above the caret.
5. Number keys, Tab, arrows, Escape, and popup clicks use the existing Quip
   commands. An accepted candidate is written through the IMK client's
   `insertText:replacementRange:` method over the tracked burst range.

The destination always owns the literal typing. Quip changes it only after an
explicit candidate selection. Moving the caret, deleting, navigating, using a
Command/Control shortcut, or leaving the input session invalidates the tracked
range rather than risking a write to the wrong location.

## Build and install

Run:

```sh
scripts/install-input-method.sh
```

The script builds a release `.app`, backs up any prior installed development
bundle under `/tmp`, installs Quip in `~/Library/Input Methods`, ad-hoc signs it,
starts it, and opens Keyboard Settings. Under **Text Input**, choose **Edit**,
add **Quip**, then select Quip from the menu-bar input menu.

The user must approve and select a third-party input source; the installer does
not modify the enabled-input-source preference behind their back.

## Current compatibility boundary

This path targets normal AppKit, WebKit, and Chromium text-input clients. Secure
fields, raw-key games, remote desktops, and editors that do not implement the
macOS text-input client contract remain out of scope. Accessibility remains in
the app for bounded window context and the explicit existing-text shortcut; it
is no longer the primary live-typing transport.
