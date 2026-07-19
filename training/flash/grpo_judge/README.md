# Quip judge-backed GRPO environment

This directory provides the judge entrypoint for a warm-started GRPO run. Build
the self-contained publish directory and publish it from Windows through WSL:

```powershell
$quipRepo = (Resolve-Path .).Path
$quipDrive = $quipRepo.Substring(0, 1).ToLowerInvariant()
$quipWslRepo = "/mnt/$quipDrive$($quipRepo.Substring(2).Replace('\', '/'))"
$quipVenv = "/home/arioo/.local/share/quip-workstream1/.venv"
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- "$quipVenv/bin/python" scripts/stage_grpo_judge_environment.py
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- "$quipVenv/bin/flash" env push --name quip-v2-context-mega-grpo-judge-20260719 .data-cache/grpo-judge-mixed
```

The staging script copies the mixed 5,240-row training corpus and every runtime
module and dependency file needed by the published environment. The source and
packaged environment both default to that mixed corpus. Lexical hints are off
for the mixed judge run and must be enabled explicitly in another config.

The reward uses deterministic schema and change-decision gates first. Exact
accepted suggestions receive full reward without a network call. Other
schema-valid candidates with the correct change decision are graded by
Freesolo-managed `Qwen/Qwen3.6-35B-A3B` serving. Judge failures fall back to
the deterministic partial reward and do not fail the training worker.

Flash injects its platform-managed `FREESOLO_API_KEY` into the worker. The
config does not declare or transmit a user-supplied judge secret.
