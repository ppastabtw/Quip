Quip model playground

This is a local-only browser surface for probing a catalog base model or
deployed Flash run alias through managed serving. It exposes the system prompt,
temperature, max tokens, and suggestion count. Each visible suggestion comes
from a separate completion using the single-suggestion JSON contract.

Runtime decision: this Windows playground uses Freesolo managed inference. It
does not represent the production or judged Quip serving path. The actual Quip
application runs the exported model and adapters locally on macOS.

From PowerShell at the repository root:

    .\training\flash\prototype\start.ps1

Then open http://127.0.0.1:8765. Enter Qwen/Qwen3.5-2B for the base model or a
deployed run alias for a trained adapter. Stop the server with Ctrl+C.

The browser never receives the Flash key. The WSL server reads the existing
Flash login and binds only to localhost. Use synthetic text, not secrets or
sensitive personal data.

Intentionally excluded: app integration, macOS capture, local inference,
streaming, authentication, persistence, history, and deployment controls.
