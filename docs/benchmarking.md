# Model benchmarking

Quip uses one held-out corpus and one scoring contract for Freesolo-served Qwen
models and user-selected Backboard models. The model matrix is
`training/flash/benchmarks/models.toml`.

The matrix includes the five supported Qwen lanes. Add each frontier model as a
new table using its exact Backboard provider and model slug:

```toml
[[models]]
label = "Your display label"
transport = "backboard"
provider = "provider-slug"
model = "model-slug"
```

Run the benchmark from Ubuntu WSL2 with the Workstream 1 environment. Start
with a live-validated dry run, which checks the Freesolo model catalog but sends no
inference requests:

```powershell
$quipRepo = (Resolve-Path .).Path
$quipDrive = $quipRepo.Substring(0, 1).ToLowerInvariant()
$quipWslRepo = "/mnt/$quipDrive$($quipRepo.Substring(2).Replace('\', '/'))"
$quipLinuxHome = (wsl -d Ubuntu -- bash -lc 'printf %s "$HOME"').Trim()
$quipPython = "$quipLinuxHome/.local/share/quip-workstream1/.venv/bin/python"

wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython scripts/run_benchmark.py --dry-run
```

Run the entire matrix after approving Backboard access:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython scripts/run_benchmark.py --allow-backboard
```

Use `--limit 2` for a smoke run or repeat `--model` to select labels from the
matrix:

```powershell
wsl -d Ubuntu --cd "$quipWslRepo/training/flash" -- $quipPython scripts/run_benchmark.py --limit 2 --model qwen-2b
```

When `--limit` is used, the run writes the exact sampled rows to `dataset.jsonl`
and scores against that subset. Unselected corpus rows are not counted as
missing predictions.

The runner requires `flash login` for Freesolo and reads `BACKBOARD_API_KEY`
from the process environment or repository `.env`. Backboard catalog and
inference access remain locked unless `--allow-backboard` is passed explicitly.
Secrets are never written to benchmark artifacts.

Every model receives the same Quip system prompt and JSON input. Freesolo sends
temperature zero, `enable_thinking = false`, and the exact JSON schema.
Backboard sends no `thinking` object, which keeps thinking disabled, and also
forces memory and web search off while requesting JSON output.

Each run writes one prediction JSONL per model plus `summary.json`, a compact
`summary.md` table, and an interactive `index.html` dashboard under
`artifacts/eval/benchmark-<timestamp>`. The dashboard includes a quality versus
latency Pareto chart, ranked quality bars, a category heatmap, and a sortable
comparison table. The artifacts
contain raw model output, request identity, latency, token usage, estimated
errors, aggregate success, correction success, unnecessary edit rate, schema
validity, latency, and category results.
They are gitignored because model responses and generated logs are local
evaluation artifacts.
