# Quip GRPO checkpoint trials

Date: 2026-07-19

## Question

Does a short warm-started GRPO continuation improve held-out Quip correction
quality over its source SFT checkpoint?

## Trial contract

- Base model: `Qwen/Qwen3.5-2B`
- SFT source run: `flash-1784436250-97876093`
- Source checkpoints: steps 40, 80, and 100
- Algorithm: GRPO
- Training examples: 200
- Training steps: 10
- Batch size: 8 prompts
- Group size: 8 completions per prompt
- Temperature: 1.0
- Structured output: strict `suggestion` JSON object
- Reward: existing dense deterministic Quip reward
- Evaluation: the same 200 held-out V2 examples for every model, five
  completions per example at temperature 0.7

The first checkpoint sweep used only the deterministic reward. The later
judge-backed lane uses Freesolo-managed Qwen3.6-35B-A3B serving and does not
require an OpenRouter secret.

## Managed runs

| Source | GRPO run | Result | Cost |
| --- | --- | --- | ---: |
| SFT step 40 | `flash-1784445486-fa199e58` | step 10 completed | $0.02095 |
| SFT step 80 | `flash-1784445488-9fee0443` | step 10 completed | $0.02095 |
| SFT step 100 | `flash-1784445493-ad3865ff` | step 10 completed after one managed infrastructure retry | $0.02095 |

Total training cost was approximately $0.06285.

## Held-out results

| Source | Overall success | Recall at five | Mean completion success | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: |
| Step 40 SFT | 57.5% | 67.0% | 55.8% | 30.0% |
| Step 40 plus GRPO | 58.5% | 69.0% | 56.8% | 20.0% |
| Step 80 SFT | 61.5% | 71.0% | 59.6% | 20.0% |
| Step 80 plus GRPO | 61.5% | 71.5% | 58.7% | 15.0% |
| Step 100 SFT | 62.0% | 70.5% | 59.3% | 15.0% |
| Step 100 plus GRPO | 61.5% | 72.0% | 59.9% | 15.0% |

Paired held-out flips were:

- Step 40: 9 examples improved and 7 regressed, exact McNemar p = 0.8036.
- Step 80: 11 examples improved and 11 regressed, exact McNemar p = 1.0.
- Step 100: 8 examples improved and 9 regressed, exact McNemar p = 1.0.

## Decision

The short deterministic-reward GRPO pass did not establish a reliable overall
quality improvement. It improved restraint for the step 40 and step 80 starts,
but correction gains and regressions balanced out. Do not promote these trial
adapters over the SFT checkpoints.

The next justified RL experiment is the prepared LLM-judge GRPO lane, using a
selected new SFT checkpoint and the same source-blind held-out evaluation.

The three trial adapters were undeployed after evaluation. The original SFT
run was restored to step 80.

## Judge-backed pilot

The judge-backed lane was validated with a 25-step pilot before scaling it.

| Field | Value |
| --- | --- |
| Run | `flash-1784449568-455d5c42` |
| Warm start | `flash-1784446044-7bc2f567/step-50` |
| Training rows available | 5,000 |
| Training horizon | 25 steps, 8 unique prompts per step |
| Samples per prompt | 8 |
| Saved checkpoints | 5, 10, and 25 |
| Training cost | $0.0549 |
| Training wall time | 295.5 seconds |

The reward first applies strict deterministic gates. Exact accepted
suggestions receive 1.0. Invalid JSON and incorrect change decisions receive
only the deterministic partial reward. A valid, non-exact suggestion with the
correct change decision is scored by Freesolo-managed
`Qwen/Qwen3.6-35B-A3B` on correction quality, meaning preservation, tone
preservation, and minimality. The four judge dimensions have weights 0.40,
0.30, 0.15, and 0.15. An unacceptable verdict is capped at 0.49. The final
judge-backed reward is `min(0.99, 0.40 + 0.59 * judge_score)`.

The pilot completed without judge, schema, or deployment failure. Its recorded
mean reward rose from 0.7039 on the first logged update to 0.9144 at step 25,
with nonzero within-group reward standard deviation. This proves that the
training loop received a usable preference signal. It does not prove held-out
improvement. A planned 1,000-row sweep was stopped before producing a result so
the team could proceed directly to the consolidated run. The pilot adapter was
undeployed.

## Consolidated 5K judge GRPO run

The source SFT checkpoint was selected from complete 1,000-row evaluations
under the five-completion ranked-candidate protocol:

| SFT checkpoint | Overall success | Recall at five | Mean completion success | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: |
| Step 50 | 71.5% | 80.0% | 69.4% | 5.0% |
| Step 100 | 75.9% | 81.7% | 73.3% | 7.0% |
| Step 150 | 76.9% | 82.8% | 75.5% | 8.0% |

Step 150 was the best fully evaluated checkpoint available at launch. The
consolidated run therefore uses `flash-1784446044-7bc2f567/step-150` as its
warm start.

| Field | Value |
| --- | --- |
| Run | `flash-1784451945-4745fb47` |
| Config | `training/flash/configs/grpo-v2-runtime-5k-judge-mega.toml` |
| Environment | `ariobarin/quip-v2-runtime-10w-5k-grpo-judge-20260719` |
| Training horizon | 625 steps, one pass over all 5,000 prompts |
| Samples per prompt | 8 |
| Total generated candidates | 40,000 |
| Saved checkpoints | 50, 100, 250, 400, and 625 |
| Estimated training cost | $1.02 |
| Estimated wall time | 1.19 hours |

This run consolidates the finalized correction prompt, strict suggestion JSON,
the runtime-weighted 5K dataset, the strongest measured SFT warm start, exact
and change-decision gates, and the 35B semantic judge into one training lane.
Checkpoint selection must still use held-out evaluation. Training reward alone
will not be used to promote the final adapter.

## Mixed context mega run

The consolidated mixed lane combines the unchanged 5,000-row runtime V2
training corpus with the 240-row training partition compiled from the approved
300-row context source. Context families remain isolated across the 240/30/30
train, evaluation, and test split.

The mixed SFT run completed before the judge continuation:

| Field | Value |
| --- | --- |
| Run | `flash-1784455014-8274f695` |
| Base model | `Qwen/Qwen3.5-2B` |
| Environment | `ariobarin/quip-v2-context-mega-20260719` |
| Training rows | 5,240 |
| Saved checkpoints | 50, 100, 150, 200, and 250 |
| Cost | $0.1490014924 |

SFT steps 150 and 250 tied at 80.00% overall success with zero unnecessary
edits on the same 60-row V2 evaluation screen. Step 250 won the complete
30-row context evaluation, 56.67% versus 40.00%, and became the GRPO warm
start.

The judge continuation used the published mixed judge environment and the
selected SFT adapter:

| Field | Value |
| --- | --- |
| Run | `flash-1784456232-84f668fe` |
| Warm start | `flash-1784455014-8274f695/step-250` |
| Environment | `ariobarin/quip-v2-context-mega-grpo-judge-20260719` |
| Judge | `Qwen/Qwen3.6-35B-A3B` |
| Training horizon | 655 steps |
| Samples per prompt | 8 |
| Saved checkpoints | 50, 100, 250, 400, and 655 |
| Estimated cost | $1.0652440171 |
| Billing state at handoff | Pending reconciliation |

The Vast worker reached 655 updates, published every required checkpoint, and
emitted a final `done` heartbeat. The provider disappeared before Flash
received its strict terminal marker, which queued a redundant retry. That
retry was cancelled after the checkpoints were verified. The run record is
therefore `cancelled`, but all five deployable checkpoint artifacts survived.

## Final selection across lanes

Checkpoint selection used the same five-completion ranked-candidate protocol
for every candidate. The compact screen contained the same 60 current V2
evaluation rows and all 30 context evaluation rows for each checkpoint.

| GRPO checkpoint | V2 overall | V2 recall at five | Context overall | Context recall at five | Context change accuracy | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Step 50 | 81.67% | 90.00% | 60.00% | 60.00% | 56.67% | 0.00% |
| Step 250 | 81.67% | 85.00% | 60.00% | 66.67% | 64.67% | 0.00% |
| Step 655 | 81.67% | 83.33% | 56.67% | 60.00% | 78.67% | 0.00% |

Step 250 is selected. It ties step 50 on top-line global and context success,
then wins on context recall at five and context change accuracy. Step 655
regresses context top-line success and is not promoted.

The selected checkpoint was evaluated once on held-out test rows after the
selection decision:

| Test split | Examples | Overall success | Recall at five | Mean completion success | Change accuracy | Schema validity | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| V2 global | 60 | 88.33% | 91.67% | 87.67% | 100.00% | 100.00% | 0.00% |
| Context | 30 | 43.33% | 60.00% | 46.00% | 70.67% | 100.00% | 25.00% |

The context test regression is a known limitation. The locked test result was
not used to reopen checkpoint selection.

Representative final-test failures show two remaining mechanisms:

- `ctx_reviewed_d872cc249359df3546c4` should keep
  `Pay the $650 deposit now`, but two of five completions copied a stale $500
  value from an old lease draft. Unchanged completions are hidden, so the
  minority changed candidate caused an unnecessary edit despite three correct
  keeps.
- `ctx_reviewed_c3d01a585483eb2b3ef7` should only correct `tomorow`, but the
  top candidate injected two doctor names from ambiguous window context. One
  exact correction was present but lost the completion vote.
- `quip_f9f4fe044ca93aac93a2` produced `new news` in a compressed-typing
  repair. The correct candidate appeared twice but lost three votes to the
  redundant repair.

These failures point to context-copy restraint and changed-versus-keep
aggregation as the next quality boundary. They do not indicate schema or
serving failures.

## Adapter handoff

| Field | Value |
| --- | --- |
| Selected adapter | `flash-1784456232-84f668fe/step-250` |
| Base model | `Qwen/Qwen3.5-2B` |
| Deployment state | `ready` |
| Deployment model | `flash-1784456232-84f668fe` |
| Immutable deployment revision | `flash-1784456232-84f668fe@step-250.5cc176c37a71ba862b5d0bb8912f3428dfb1f4a3` |
| Canonical export | `ppasta/quip-v2-context-mega` |
| Canonical export revision | `297ac9b68fce60ff34a9d415dea7d0376441e9a0` |
| Checkpoint export | `ppasta/quip-v2-context-mega-grpo-step250` |
| Checkpoint export revision | `03d4de4fb48992a1a23c3465fd2f63c62c5a0d5b` |
| Export state | `exported`, private repositories |
| Evaluation protocol | `five_completion_ranked_candidates_v1` |

Identity hashes:

| Artifact | SHA-256 |
| --- | --- |
| Training prompt package | `8569bbf9932a222ee45e4a9350c187057e51b5cf525d18407049c9ed6d34fd64` |
| Shared prompt Git blob | `d293856e6a38f9179e2e719c4b015f697d82de64` |
| Mixed 5,240-row train | `e67d1aadb9ce11177d287bfb02c367ca71f1a7bf995a72342bae6cf12c3ea211` |
| Approved 300-row context source | `db399c3ec92d7b17f587c64bf9e1de0bddc536ac2edbf08c6582fe03d315f225` |
| Full V2 evaluation split | `2284458cc91e59700ad1b64636562bda3df26e657db8e90791594d519a264178` |
| 60-row V2 evaluation screen | `8f724b85cedd153640e1a83d7a72ed380b7033a4b95fc626b2b21f638a865c30` |
| Full context evaluation split | `721a58404122389b558fb5b6422bd2a2f0f1a59d8bfe485c12d914ffda32abf8` |
| 30-row context evaluation screen | `8d4b4e284969abc8216837abf22e71c0c459980dcc9c159da1482e873b71064a` |
| Full V2 test split | `df750f68b809aef2a14e5dba2d327ee3419995be388fb6e42540b41c2971db88` |
| 60-row V2 final test | `7417cba71f315c834fa1179b8103543bb4af10b0b73734be84d9882003e731b0` |
| Full context test split | `0ffd58ce11b034ef83be8f572258ebcc6fb7f5445e9820c56a9bf16c05718d1f` |
| 30-row context final test | `8b97bdce494795d401759292a2aa43e4b243c212cf0550e3826a952c66e745b0` |

Generated predictions, model binaries, adapters, logs, caches, and secrets are
excluded from Git. The two Hugging Face exports are the durable model
artifacts.
