---
name: validate-quip-context
description: Validate Quip Native keyboard context ingestion through the real macOS Accessibility and native-IME bridge boundaries. Use after changing Accessibility window-text collection, native capture context policy, context debug events, model request context wiring, or supported-app context behavior.
---

# Validate Quip Context

Run the real-app validator from the repository root:

```bash
.agents/skills/validate-quip-context/scripts/validate.sh
```

The validator builds the Tauri app, launches it with fixture inference and
explicit local debug text, opens synthetic marker content in isolated TextEdit
and Chrome processes, sends captures through the native InputMethodKit loopback
protocol, and verifies that each bounded Accessibility snippet appears in the
capture, prediction request, and inference-result metadata.

The run temporarily activates TextEdit and Chrome and requires Accessibility
permission for Quip. It must not kill an existing Quip process or print the
captured snippet. A successful run ends with `Quip native context integration
passed`. Treat unit tests or the fake Slack demo as insufficient evidence.
