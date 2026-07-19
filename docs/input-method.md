# Quip native input method

Quip's live typing path uses a small standalone InputMethodKit bundle for
ordinary macOS text fields. The bundle passes literal typing through to the
destination, tracks a bounded UTF-16 burst and its caret rectangle, and sends
only that bounded capture to the running Tauri app over loopback TCP.

The Tauri app remains the owner of composition state and local inference. In
live mode it sends the capture through the persistent Rust sidecar to the local
Base or Global MLX endpoint. The native input method receives only settled,
navigation, dismissal, and commit messages. A commit is applied with the
active text client's `insertText:replacementRange:` operation after the client
range is revalidated.

The non-focusable candidate bar is always-on-top and visible across macOS
workspaces. Invalid native caret geometry falls back to the focused element's
Accessibility bounds so Notes cannot place the bar off-screen.

The Tauri app always traverses the focused supported window's Accessibility
tree, collects up to 240 characters of visible static text, excludes editable
controls, and includes the bounded snippet in the prediction request. TextEdit
falls back to its active document text because the document is itself the
editor. Apple Notes reads its native active editor, removes the line containing
the caret, and bounds the remaining note text; its window title is replaced by
a non-content label so the current line cannot leak through preview metadata.
Local debug events record the snippet title and text for inspection.

```text
TextEdit / Notes / browser text field
  -> Quip Native (InputMethodKit)
  -> 127.0.0.1:48731
  -> Tauri composition engine
  -> persistent inference sidecar
  -> local MLX Base or Global endpoint
  -> candidate bar
  -> explicit selection
  -> InputMethodKit replacement in the original field
```

Ordinary text, dismissal, stale predictions, and zero-candidate results do not
replace destination text. Moving the selection before commit invalidates the
tracked replacement range.

## Build and install

Build the bundle without changing system state:

```sh
npm run build:input-method
```

Install it as a macOS input source:

```sh
npm run install:input-method
```

The installer builds and signs a staged bundle, backs up an earlier Quip input
method under `/tmp`, installs `Quip Native.app` in `/Library/Input Methods`,
registers it with Text Input Services, restarts the old input-method host, and
selects Quip Native. It uses an ad-hoc local signature by default; set
`QUIP_CODESIGN_IDENTITY=auto` to use an available Apple development identity.
macOS requires administrator approval.

For live model inference, start the model services and Tauri app before typing:

```sh
src-tauri/sidecars/inference/scripts/run-live-app.sh
```

The live launcher starts Quip tray-only; it does not open the demo harness.
Open the demo explicitly from the tray menu or set `QUIP_SHOW=demo` when it is
needed for development.

The input method reconnects automatically to the app's loopback bridge. If the
app is installed in `/Applications`, the input method can launch it when the
bridge is unavailable; a development run should normally be started explicitly
so its live backend and model-variant environment are unambiguous.

Validate the real Accessibility/native-bridge boundary in TextEdit, Notes, and
Chrome:

```sh
.agents/skills/validate-quip-context/scripts/validate.sh
```

## Compatibility boundary

This path targets standard AppKit, WebKit, and Chromium text clients. Secure
fields, raw-key and canvas editors, games, remote desktops, terminals, and
clients that do not implement the macOS text-input contract remain out of
scope. Accessibility remains responsible for bounded open-window context and
the secondary existing-text path.
