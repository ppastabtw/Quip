Quip model playground

This is a local-only browser surface for probing a catalog base model or
deployed Flash run alias through managed serving. It exposes the system prompt,
temperature, max tokens, and completion count. A run requests up to five
separate completions using the single-suggestion JSON contract. Exact draft
outputs are skipped. Identical changed strings are grouped, then unique
candidates are ranked by vote count and earliest completion index. The page
shows each vote count and treats an all-unchanged result as a successful skip.

The QWERTY augmentation lab and model playground are separate tabs, with only
the active tab visible. The lab creates deterministic typing errors without
calling a model. The page starts with a random seed, which can be edited to
reproduce a result. Enter correct text, an error count, and relative operator
weights. Each operator label has a short explanation. The response includes the
corrupted draft and an operation trace. The same inputs reproduce the same
result. Use as model draft copies it into the
managed inference form and switches tabs. Reset defaults restores the tunable
lab profile.

Runtime decision: this Windows playground uses Freesolo managed inference. It
does not represent the production or judged Quip serving path. The actual Quip
application runs the exported model and adapters locally on macOS.

From PowerShell at the repository root:

    .\training\flash\prototype\start.ps1

Then open http://127.0.0.1:8765. Enter Qwen/Qwen3.5-2B for the base model or a
deployed run alias for a trained adapter. Stop the server with Ctrl+C.

The browser never receives the Flash key. The WSL server reads the existing
Flash login and binds only to localhost. Use synthetic text, not secrets or
sensitive personal data. The augmentation endpoint does not read Flash login.

Intentionally excluded: app integration, macOS capture, local inference,
streaming, authentication, persistence, history, and deployment controls.
