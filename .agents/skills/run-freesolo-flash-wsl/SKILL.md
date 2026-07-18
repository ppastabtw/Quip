---
name: run-freesolo-flash-wsl
description: Run and validate this repository's Freesolo Flash training lane from Windows through Ubuntu WSL2. Use for Flash setup, authentication, environment publication, cost checks, training, checkpoint evaluation, deployment, export, or Windows fcntl import failures in hackthe6ix.
---

# Run this project's Freesolo lane through WSL

## Keep the boundary explicit

- Run `freesolo-flash` in Ubuntu WSL2, not native Windows Python. Flash 1.0.0 imports the POSIX-only `fcntl` module.
- Keep the Python environment in the Linux filesystem and the project source in the mounted Windows checkout.
- Use `training/flash` as the working directory for project commands.
- Never patch the installed Flash package to bypass platform imports.
- Never commit credentials, datasets containing personal records, predictions, logs, adapters, or model binaries.

## Resolve project paths

From the repository root in PowerShell:

```powershell
$quipRepo = (Resolve-Path .).Path
$quipDrive = $quipRepo.Substring(0, 1).ToLowerInvariant()
$quipRest = $quipRepo.Substring(2).Replace('\', '/')
$quipWslRepo = "/mnt/$quipDrive$quipRest"
$quipLinuxHome = (wsl -d Ubuntu -- bash -lc 'printf %s "$HOME"').Trim()
$quipVenv = "$quipLinuxHome/.local/share/hackthe6ix-workstream1/.venv"
$quipFlash = "$quipVenv/bin/flash"
$quipPython = "$quipVenv/bin/python"
```

Do not hard-code a Windows clone path or Linux username.

## Bootstrap the environment

Inspect WSL first:

```powershell
wsl --status
wsl -d Ubuntu -- bash -lc 'command -v uv; command -v python3; printf "%s\n" "$HOME"'
```

Create the Linux virtual environment with the project-tested versions:

```powershell
wsl -d Ubuntu -- bash -lc 'QUIP_VENV="$HOME/.local/share/hackthe6ix-workstream1/.venv"; UV_BIN="$(command -v uv)"; "$UV_BIN" venv "$QUIP_VENV" --python 3.12; "$UV_BIN" pip install --python "$QUIP_VENV/bin/python" freesolo-flash==1.0.0 freesolo==0.2.56 httpx==0.28.1'
```

`freesolo-flash` provides the CLI. `freesolo` provides the SDK imported by `environment.py`.

## Authenticate safely

Run `whoami` first:

```powershell
wsl -d Ubuntu -- $quipFlash whoami
```

If login is required, open an interactive Ubuntu shell and enter the key there:

```bash
source "$HOME/.local/share/hackthe6ix-workstream1/.venv/bin/activate"
read -rsp "Freesolo API key: " FREESOLO_API_KEY
echo
export FREESOLO_API_KEY
flash login
unset FREESOLO_API_KEY
```

Never print the key or pipe it from PowerShell. A carriage return can invalidate the header and cause an error to reproduce the credential. If a credential appears in output, tell the user to rotate it immediately.

## Validate before spending

Run the local gates:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython scripts/validate_datasets.py
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython -m unittest discover -s tests -v
wsl -d Ubuntu -- $quipFlash --version
wsl -d Ubuntu -- $quipFlash whoami
wsl -d Ubuntu -- $quipFlash models
```

For every paid run, run both server validation and the local cost estimate first:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipFlash train configs/sft.toml --dry-run
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipFlash train configs/sft.toml --cost
```

Show the quote and obtain approval before submitting paid training.

## Run and evaluate training

Publish a changed environment before training against it:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipFlash env push --name quip .
```

Submit and inspect a run:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipFlash train configs/sft.toml
wsl -d Ubuntu -- $quipFlash status <run-id>
wsl -d Ubuntu -- $quipFlash log <run-id>
wsl -d Ubuntu -- $quipFlash checkpoints <run-id>
```

Evaluate useful checkpoints on the untouched split. Do not promote the last checkpoint automatically. Deploy one checkpoint at a time, run `scripts/run_managed_eval.py`, score it with `scripts/evaluate_predictions.py`, inspect real outputs, and undeploy rejected adapters.

Use GRPO only after an SFT checkpoint beats the base evaluation. Keep per-user profile runs separate from the global held-out split and exclude raw drafts, ambient context, secrets, and unconfirmed records.

## Finish with evidence

Report:

- Flash and SDK versions
- authenticated account status without key material
- environment id and run id
- quoted and charged cost
- checkpoint evaluated
- held-out metrics and inspected failure cases
- deployment or export state
- any endpoint that still needs undeployment

Do not claim macOS inference, Metal adapter loading, or application integration from this Windows lane.
