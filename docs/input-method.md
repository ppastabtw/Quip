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
method under `/tmp`, installs `Quip Native.app` in `/Library/Input Methods`, and
registers it with Text Input Services. macOS requires administrator approval.
After installation, add **Quip Native** under Keyboard Settings → Text Input →
Edit, then select it from the input menu.

For live model inference, start the model services and Tauri app before typing:

```sh
src-tauri/sidecars/inference/scripts/run-live-app.sh
```

The input method reconnects automatically to the app's loopback bridge. If the
app is installed in `/Applications`, the input method can launch it when the
bridge is unavailable; a development run should normally be started explicitly
so its live backend and model-variant environment are unambiguous.

## Compatibility boundary

This path targets standard AppKit, WebKit, and Chromium text clients. Secure
fields, raw-key and canvas editors, games, remote desktops, terminals, and
clients that do not implement the macOS text-input contract remain out of
scope. Accessibility remains responsible for bounded open-window context and
the secondary existing-text path.
