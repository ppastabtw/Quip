# Hybrid lexical correction experiment

## Decision

Use an opt-in two-layer correction protocol:

1. Generate bounded dictionary candidates from the original text.
2. Give the model both the unchanged original text and those candidates.

The dictionary layer never applies edits. The model can choose a candidate,
ignore every candidate, or return the original text unchanged. This preserves
Quip's explicit candidate-selection safety model.

## Input contract

The hybrid user message adds one field to the existing model input:

```json
{
  "text": "hteir going tommorow",
  "lexical_hints": [
    {"token": "hteir", "candidates": ["their", "heir"]},
    {"token": "tommorow", "candidates": ["tomorrow"]}
  ]
}
```

`lexical_hints` is always an array in hybrid mode and may be empty. The output
contract remains exactly `{ "suggestion": "..." }`.

## Candidate generation

`training/flash/lexical_candidates.py` builds a 30,000-word English index from
`wordfreq==3.1.1`. A BK-tree retrieves candidates by ordinary Levenshtein
distance. Weighted Damerau-Levenshtein distance then reranks them, discounting
adjacent QWERTY substitutions and transpositions. The generator:

- keeps at most three candidates per suspicious token;
- preserves the original text;
- skips already-common dictionary words;
- skips likely names away from sentence start;
- skips paths, handles, underscored identifiers, version-like strings, and
  user-protected words;
- does not inspect the row target or accepted suggestions.

The same module enriches Freesolo training prompts and managed evaluation
prompts through the finalized compact model-input serializer. That serializer
keeps only `context_snippets`, `text`, and the hybrid `lexical_hints` field.
Hybrid SFT is enabled with `lexical_hints = true` in the environment parameters,
preventing training and managed evaluation input mismatch.

## Evidence

Protocol: 200-row V1 evaluation split, five completions per example, vote-ranked
candidates, Qwen3.5 2B, and the checked-in 2,000-row training split. The V1 data
is a narrow synthetic QWERTY benchmark, so these scores do not establish
performance on shorthand, phonetic spelling, or local macOS inference.

| Lane | Split | Overall | Recall at 5 | Correction success | Unnecessary edits | Schema | Mean managed latency |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Base model, original input | eval | 48.5% | 57.0% | 51.7% | 80.0% | 100% | 1581 ms |
| Base model, lexical hints | eval | 61.0% | 66.5% | 63.9% | 65.0% | 100% | 1790 ms |
| Hybrid SFT, step 25 | eval | 73.0% | 78.0% | 70.6% | 5.0% | 100% | 1900 ms |
| Hybrid SFT, step 50 | eval | 76.0% | 78.0% | 75.6% | 20.0% | 100% | 1434 ms |
| Hybrid SFT, step 25 | locked test | 78.5% | 81.5% | 76.7% | 5.0% | 100% | 1436 ms |

The lexical generator produced hints for 68.3% of changed evaluation examples.
A gold target token appeared among its candidates for 60.0% of changed examples.
It produced no hints for the 20 unchanged examples.

Step 25 is selected over step 50. Step 50 gained three evaluation points but
quadrupled unnecessary edits from 5% to 20%. Quip's conservative correction
contract makes the earlier checkpoint the better tradeoff. Managed latency is
reported for observation but was measured sequentially against a shared service
and should not be treated as a local-device benchmark.

## Freesolo artifacts

- Environment: `ariobarin/quip-hybrid-lexical`
- Successful run: `flash-1784447965-d1476ad3`
- Cost: `$0.03333654848484849`
- Selected adapter: `flash-1784447965-d1476ad3/step-25`
- Rejected checkpoint: `flash-1784447965-d1476ad3/step-50`
- Failed zero-cost preflight run: `flash-1784447684-679734d3`
- Failure cause: remote worker lacked `wordfreq`
- Repair: `wordfreq==3.1.1` declared in `[environment].pip`

The selected adapter remains stored in Freesolo. Export to `ariobarin/quip` was
not completed because WSL had no Hugging Face token. The managed serving alias
was deregistered after evaluation.

## Runtime handoff

Workstream 2 should port the deterministic candidate generator or consume an
equivalent prebuilt dictionary index, construct the exact `lexical_hints` field,
and use `training/flash/system_prompt_hybrid.txt` byte-for-byte. The raw draft
must remain the `text` value, and empty hints must not bypass model inference.
Local macOS latency and candidate behavior remain unverified from this Windows
worktree.
