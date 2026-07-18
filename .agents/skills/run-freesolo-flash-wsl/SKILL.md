---
name: run-freesolo-flash-wsl
description: Run Quip Freesolo Flash work from Windows through Ubuntu WSL2. Use for setup, authentication, environment publication, training, checkpoint evaluation, deployment, export, or native Windows fcntl failures.
---

# Run Freesolo Flash through WSL

## Respect the boundary

- Run Flash in Ubuntu WSL2 because Flash 1.0.0 imports POSIX-only `fcntl`.
- Keep the virtual environment in Linux and use the Windows checkout through `/mnt/<drive>`.
- Work from `training/flash`. Do not patch the installed package.
- Never commit secrets, personal data, predictions, logs, adapters, or model binaries.

## Set up once

Resolve paths from the repository root in PowerShell:

```powershell
$quipRepo = (Resolve-Path .).Path
$quipDrive = $quipRepo.Substring(0, 1).ToLowerInvariant()
$quipWslRepo = "/mnt/$quipDrive$($quipRepo.Substring(2).Replace('\', '/'))"
$quipLinuxHome = (wsl -d Ubuntu -- bash -lc 'printf %s "$HOME"').Trim()
$quipVenv = "$quipLinuxHome/.local/share/quip-workstream1/.venv"
$quipFlash = "$quipVenv/bin/flash"
$quipPython = "$quipVenv/bin/python"
```

After resolving the paths, invoke Linux executables directly through WSL. This
keeps PowerShell from consuming Linux shell variables:

```powershell
wsl -d Ubuntu -- $quipFlash --version
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython scripts/validate_datasets.py
```

Use `bash -lc` only when the operation actually needs Linux shell syntax.

If the environment is absent, create it with the project-tested versions:

```powershell
wsl -d Ubuntu -- bash -lc 'V="$HOME/.local/share/quip-workstream1/.venv"; U="$(command -v uv)"; "$U" venv "$V" --python 3.12; "$U" pip install --python "$V/bin/python" freesolo-flash==1.0.0 freesolo==0.2.56 httpx==0.28.1'
```

Authenticate only inside an interactive WSL shell. Read the key silently, export it for `flash login`, then unset it. Never print or pipe a key from PowerShell. If a key appears in output, require rotation.

## Operate the training lane

1. Run `scripts/validate_datasets.py`, all tests, `flash --version`, `flash whoami`, and `flash models` before training.
   Keep JSONL `input` and `output` values structured when they are JSON. Freesolo serializes structured values into model-facing text.
2. Publish `ariobarin/quip` again only when the environment or packaged dataset changes.
   If a remote run fails before model load because its published runtime path is missing, republish once and resubmit unchanged.
   If read-only control calls time out while workers keep heartbeating, back off and retry the read without resubmitting training.
3. Run `flash train <config> --dry-run` before submission. Obtain approval for
   paid work unless the active goal already records an explicit user waiver.
4. Use explicit checkpoints. Inspect with `flash status <run-id>`, stream with
   `flash log -f <run-id>`, and list saved adapters with
   `flash checkpoints <run-id>` after training.
5. Evaluate promising checkpoints on the untouched split with `run_managed_eval.py` and `evaluate_predictions.py`. Inspect outputs and undeploy rejected adapters. Never select the final checkpoint automatically.
6. Use GRPO only after SFT beats the base evaluation. Keep profile runs separate from the global held-out set, then export the chosen adapter promptly.

## Report evidence

Report versions, authenticated status without key material, environment and run ids, cost, checkpoint, held-out metrics, inspected failures, and deployment or export state. Do not claim macOS inference, Metal loading, or app integration from Windows.
