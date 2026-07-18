# A deep dive into post-training with Flash

This is a slide-by-slide transcription of the 46-page Freesolo presentation formerly referenced at `tmp/pdfs/Freesolo_Post_Training_Slides.pdf`. It preserves the wording, numbers, equations, code, tables, order, and meaning-bearing layout of the source.

## Presentation conventions

- Author: David Shan, Co-founder @ Freesolo.
- Recurring footer: `POST-TRAINING WITH FLASH`.
- Blue or green uppercase text is represented as bold text where its emphasis matters.
- Letter-spaced display text is written as ordinary words without artificial spaces between letters.
- Decorative pixel blocks are noted on the cover, section dividers, and closing slide. Purely decorative borders and backgrounds are omitted.

## Slide 1

**Cover**

Freesolo

> A deep dive into<br>
> post-training with<br>
> Flash.

David Shan<br>
Co-founder @ Freesolo

Layout: dark navy background with interlocking blue, green, and black pixel blocks along the right side.

## Slide 2

**Contents**

# What we'll cover.

| Section | Topic | Details |
| --- | --- | --- |
| 01 | Why post-train | Mid-training, the economics of small models, and the performance gap. |
| 02 | Environments | The core abstraction: your task as code, one object for training, evals, and data. |
| 03 | How models learn | Thinking · LoRA · distillation · on/off-policy · KL · how SFT, OPD, and GRPO actually work. |
| 04 | Flash | The stack, the model catalog, the managed loop, and serving the trained model. |

## Slide 3

**Overview**

# The outer loop.

1. **01 · BUILD**

   Write the environment: your task, your data, your reward.

2. **02 · EVALUATE**

   Baseline the base model on a held-out split. That's the floor.

3. **03 · TRAIN**

   SFT teaches the format, then GRPO or OPD sharpens it. Eval every checkpoint.

4. **04 · DEPLOY**

   Ship when the eval says so. Retrain the same config as the task drifts.

The four cards are connected left to right by arrows.

```console
$ flash train config.toml       # the sequence of a real project: measure between every step
```

## Slide 4

**Section 01**

# 01

# Why post-train.

Layout: dark navy section divider with blue, green, and black pixel blocks on the right.

## Slide 5

**01 Why post-train**

# Pre-training and mid-training.

Both happen before you ever touch the model, and both are the provider's job. You inherit them through base-model choice.

### Phase 01 · Pre-training

**Fluent, but generic.**

Next-token prediction over trillions of web-scale tokens. Good at everything in general, nothing in particular.

### Phase 02 · Mid-training

**Same loss, sharper data.**

A curated diet: domain corpora, longer documents, annealing. It reshapes what the model knows, not how it behaves.

## Slide 6

**01 Why post-train**

# Post-training.

This phase is yours: it turns the generalist into a specialist on your task, with your data and your definition of a good answer.

### Phase 03 · Your job

Driven by your environment: the prompts the model practices on, and the score that says what counts as a good answer.

### Three ways to teach

SFT imitates examples, OPD distills a teacher, GRPO practices against a reward. One config line switches.

### Behavior into weights

Formats, tone, and judgment move out of the prompt and into the model, and it's small enough to rerun as the task drifts.

Process:

`PRE-TRAINING` → `MID-TRAINING` → `BASE MODEL` → `POST-TRAINING` → `YOUR MODEL`

The `BASE MODEL` step is dark navy and the `YOUR MODEL` step is green.

## Slide 7

**01 Why post-train**

# Why post-train at all?

### 01 · Use a smaller model

A sub-10B model tuned on your data can beat a frontier model on your narrow task, at a fraction of the cost and latency.

### 02 · It's how the big labs win

The gap between a raw base and ChatGPT is post-training. o1-style reasoning is a post-training result, not a bigger pre-train.

### 03 · Consistency beyond prompting

Prompting gets close but not consistent. Post-training moves behavior into the weights: stable formats, stable tone, fewer escapes.

## Slide 8

**01 Why post-train**

# The performance difference.

| Model and setup | Held-out accuracy |
| --- | ---: |
| Qwen3.5-4B · base, zero-shot | 56% |
| Frontier model · zero-shot | 71% |
| Qwen3.5-4B · SFT → GRPO on-task | 89% |

Callouts:

- **+18 pts** over the frontier model.
- **5 hrs** agent → deployable model.
- **89%** on-task accuracy, held-out eval.

Caption: Accuracy on a held-out eval · Flash benchmark task (freesolo.co).

## Slide 9

**Section 02**

# 02

# Environments.

Layout: dark navy section divider with blue, green, and black pixel blocks on the right.

## Slide 10

**02 Environments**

# What is an environment?

Your task, expressed as code: the single source of truth for what the model practices on and how it's graded.

Author it locally, publish with `flash env push`, reference it by id from your config.

### Component 01

**The dataset.**

The prompts your model practices on, plus any gold answers, packaged as simple input/output records (`train.jsonl`).

### Component 02

**The reward.**

A scoring function: it looks at the model's answer and returns a score. Gold-answer check, or any code you can write.

## Slide 11

**02 Environments**

# One environment, three jobs.

Build the task once and the same object powers the whole workflow. It's the first thing you write, not the last.

### Train

SFT, GRPO, and OPD are driven by the same environment. Switching algorithms is one line of config.

### Evaluate

Score any model on the same held-out task: the base, a frontier model, an OPD teacher, or every checkpoint of a run.

### Generate data

Rollouts through the environment are synthetic training data: scored traces you can filter, study, and reuse.

Layout: `TRAIN` and `EVALUATE` are parallel cards. `GENERATE DATA` is centered beneath them.

## Slide 12

**02 Environments**

# Evals.

An evaluation turns "it seems better" into a measurement, on prompts training never saw.

### Held-out split

Score the model on prompts kept out of training. Gains that only show up on training data are memorization, not learning.

### Before and after

Run the same eval on the base, the teacher, and every checkpoint. That's how "+18 pts" is claimed, and how a regression is caught.

### Evals ≠ rewards

The reward gets optimized; the eval gets trusted. If they're the same thing, reward hacking looks exactly like progress.

## Slide 13

**02 Environments**

# Eval statistics.

One eval number is a sample, not a fact. The same checkpoint, run twice, can land a couple of points apart.

Know your noise floor before celebrating.

### Mean reward vs pass@k

Mean reward tracks average quality; pass@k asks whether any of k attempts succeeds. Agents and codegen usually care about pass@k.

### Run-to-run variance

Rerun the same eval on the same checkpoint; the gap is your noise floor. One seed's +2 points inside that band is not a result.

### Sample size

A 50-prompt eval swings several points on luck alone. Grow the held-out split until the deltas you care about beat the noise.

## Slide 14

**02 Environments**

# Reward hacking.

Optimize a proxy hard enough and it stops measuring what you meant. In RL this is the default failure mode.

### The reward you wrote

> "Score support replies higher when they cite the right policy section and stay under 80 words."

### The policy it learned

> Terse keyword lists that name the section and stop. Reward: perfect. Feature: useless.

### Held-out eval

A measurement the optimizer never touches. Reward climbing while the eval is flat means you're being hacked.

### Read the traces

High-reward rollouts that look wrong to you are the alarm going off. Fix the reward, not the model.

### The KL leash

The β·KL penalty keeps the policy near the reference, so degenerate-but-high-scoring text costs something.

Layout: the written reward and learned policy are paired across the top. The three mitigations form a row underneath.

## Slide 15

**Section 03**

# 03

# How models learn.

Layout: dark navy section divider with blue, green, and black pixel blocks on the right.

## Slide 16

**03 How models learn**

# Thinking vs non-thinking models.

OpenAI's o1: use RL to teach a model to think before answering. Accuracy started scaling with thinking time, not just size.

### Non-thinking

**Answers immediately.**

Lowest latency and cost. The default for high-volume tail work and for guided decoding, which binds from the first token.

### Thinking

**Reasons first, then answers.**

Emits reasoning tokens first. Wins on math, code, and multi-step work, but you pay latency for every thought.

```toml
thinking = true     # opt in per run (thinking-capable bases)
# a deployed adapter keeps the thinking mode it was trained with
```

## Slide 17

**03 How models learn**

# What is a LoRA adapter?

Train small add-on matrices and keep the base frozen. The adapter is the diff between the generalist and your specialist.

Composition:

`BASE MODEL` + `LoRA ADAPTER` = `YOUR MODEL`

| Part | State |
| --- | --- |
| Base model | frozen · billions of params |
| LoRA adapter | trained · megabytes |
| Your model | specialized · deployable |

### Cheap & fast

You update a tiny fraction of the parameters, so runs are quick and quotable.

### Portable

Megabytes, not gigabytes. Export the adapter to your own HuggingFace repo.

### Efficient to serve

Many adapters that share a base model can be served on the same GPUs.

```toml
lora_rank = 32              lora_alpha = 64       # two lines of [train] config
```

## Slide 18

**03 How models learn**

# LoRA math.

B and A are the only weights that train. W never moves; the update is their product, scaled by α/r.

> W′ = W + (α/r) · B·A

B is d×r and A is r×k, so the update has rank at most r; α sets its strength.

### 01 · Where it attaches

Adapters wrap the attention projections (Q, K, V, O) and often the MLP. Everything else stays frozen.

### 02 · What rank buys

r caps what the update can express. Formats need little (8-32); new skills and RL headroom want more.

### 03 · What alpha does

The α/r factor sets how loudly the adapter speaks over the base; α = 2r is the usual default.

### 04 · The two config lines

`lora_rank = 32`, `lora_alpha = 64` from the last slide; the model catalog caps rank per base.

## Slide 19

**03 How models learn**

# What is distillation?

Compress a strong teacher into a small student. The student never sees weights, only behavior.

### Classic · off-policy

**Imitate the teacher's outputs.**

The teacher generates; the student imitates via SFT. Simple and scalable, but the student is never corrected on its own mistakes.

### OPD · on-policy

**Teacher grades the student.**

The student generates; the teacher grades every token of that attempt. Dense signal, on exactly the states the student visits.

## Slide 20

**03 How models learn**

# On-policy vs off-policy.

The most useful axis in post-training: did the data come from somewhere else, or from the model itself?

### Off-policy

**Learn from data produced elsewhere.**

Curated gold answers, a teacher's outputs, pre-collected preferences. SFT, classic distillation, DPO.

- **+** Simple, cheap, teaches brand-new formats from scratch.
- **−** Distribution mismatch: the model is never trained on its own mistakes.

### On-policy

**Learn from the model's own outputs.**

Completions are sampled from the current model during training, then graded. GRPO, OPD.

- **+** Fixes the errors the model actually makes; gains compound.
- **−** Needs a grader (a reward or a teacher) plus generation compute during training.

## Slide 21

**03 How models learn**

# SFT vs OPD vs GRPO.

| | SFT | OPD | GRPO |
| --- | --- | --- | --- |
| **Learning mode** | Learn by imitation | Learn from a teacher | Learn by practice |
| **Signal** | Imitate gold answers, token by token. | A teacher grades the student's own tokens: dense, per-token. | One sparse reward per completed rollout. |
| **You supply** | Prompt → answer pairs in your dataset. | Just a teacher (GLM 5.2 default). No answers, no reward. | A reward function that separates good answers from bad. |
| **Policy** | Off-policy. | On-policy. | On-policy. |
| **Ceiling** | Your dataset. It can't fix what your examples never show. | The teacher. Training only pulls you toward it. | Whatever your reward can measure. It can pass any teacher. |
| **Pick it when** | You have examples. Teach the output contract (format, tone, tool syntax) first. | A stronger model already does the task. Warm-start from SFT (`init_from_adapter`). | You can score outputs you couldn't write, and some rollouts already succeed. |

## Slide 22

**03 How models learn**

# What is KL divergence?

One number for how different two probability distributions are: the extra surprise from betting on Q when reality follows P.

> KL( P ‖ Q ) = Σ P(x) · log P(x)/Q(x)

Summed over every outcome x; in an LLM, x ranges over the token vocabulary.

### 01 · Extra surprise

How much likelier P makes each outcome than Q, weighted by how often P produces it.

### 02 · Zero means same

KL = 0 only when the distributions match exactly; no upper bound as they drift apart.

### 03 · Not symmetric

KL(P ‖ Q) ≠ KL(Q ‖ P). Forward KL punishes missing P's modes; reverse KL punishes inventing things P never does.

### 04 · Per token, for LLMs

For models, P and Q are next-token distributions; KL is computed at each position and averaged.

## Slide 23

**03 How models learn**

# Forward KL.

KL( P ‖ Q ) with reality in front, model behind. Minimizing it makes the model cover everything the data actually does.

> min KL( P_data ‖ Q_model )

Identical, up to a constant, to the cross-entropy loss SFT already minimizes.

### 01 · Mode-covering

P leads: wherever the data puts probability, the model must too, or the penalty explodes.

### 02 · This is SFT

Teacher forcing maximizes the likelihood of gold tokens, which minimizes forward KL to the data.

### 03 · The cost

Covering every mode can over-spread a small model: it hedges across styles instead of committing.

### 04 · When it fits

You have gold answers and want the full range of the data's behavior imitated.

Visual: a small plot contrasts a multi-peaked `P` distribution with a wider `Q` curve. Caption: Q spreads to cover each P's mode.

## Slide 24

**03 How models learn**

# Reverse KL.

KL( Q ‖ P ) with the model in front, teacher behind. Minimizing it makes the student commit to what the teacher does best.

> min KL( Q_student ‖ P_teacher )

Computed per token on the student's own samples; this is OPD's training objective.

### 01 · Mode-seeking

Q leads: the student is punished for anything the teacher wouldn't produce. It picks a mode and commits.

### 02 · This is OPD

The teacher grades every token of the student's own attempt; the gradient pulls the student toward it.

### 03 · On its own states

Samples come from the student, so credit lands exactly on the mistakes it actually makes.

### 04 · When it fits

A stronger model already does the task and you want a small model to match it.

Visual: a small plot shows narrow `Q` concentrated on one of the modes in `P`. Caption: Q commits to one of P's modes.

## Slide 25

**03 How models learn**

# KL divergence.

How far one distribution has drifted from another. Post-training uses the same quantity in two opposite roles.

### As a penalty · GRPO, RLHF

**Stay close to yourself.**

A β·KL(π ‖ π_ref) term keeps the policy near the reference. Too loose: reward hacking. Too tight: the model can't move.

### As the objective · OPD

**Move toward the teacher.**

OPD minimizes reverse KL to the teacher, per token. Here drift isn't punished; it is the whole training signal.

## Slide 26

**03 How models learn**

# What is entropy?

The average surprise of a distribution. Confident and peaked scores low; spread-out and unsure scores high.

> H( P ) = − Σ P(x) · log P(x)

Measured in bits or nats; for an LLM, one value per next-token distribution.

### 01 · Confidence gauge

Zero when one token gets all the probability; maximal when every token is equally likely.

### 02 · Temperature

Sampling temperature scales entropy: higher spreads probability out, lower sharpens it.

### 03 · Exploration

RL needs entropy: varied rollouts are what GRPO compares. No variety, no group signal.

### 04 · Entropy collapse

Optimization drains entropy over a run; if it hits the floor, rollouts converge and learning stalls.

## Slide 27

**03 How models learn**

# Reward design.

### 01 · Verifiable

A checkable fact: exact match, tests pass, the SQL runs. Cheapest and hardest to fool. Use it wherever ground truth exists.

### 02 · Rubric

Code that scores properties: valid JSON, cites the right section, under the cap. Partial credit shapes early learning.

### 03 · LLM-as-judge

A model grades what code can't: tone, helpfulness, reasoning. Most flexible, easiest to hack; pin it to a rubric and spot-check.

## Slide 28

**03 How models learn**

# How SFT works.

Teacher forcing: show a gold answer, train the model to reproduce it token by token. Still the right first move.

> loss = − Σ log p( gold token | prefix )

Cross-entropy on the answer tokens; prompt tokens are masked out of the loss.

### 01 · Show the answer

Each training row is a prompt plus the exact answer you want back.

### 02 · Predict every token

The model predicts each answer token given the prompt and the gold tokens before it.

### 03 · Penalize the misses

Cross-entropy pushes probability onto every gold token the model got wrong.

### 04 · Sweep the dataset

Repeat for epochs over your rows; the LoRA weights absorb the format.

## Slide 29

**03 How models learn**

# SFT data.

A format task needs hundreds of clean examples, not millions. Past a point, more rows teach less than better rows.

Coverage and hygiene beat raw volume.

### Hundreds, not millions

A few hundred representative examples teach a format; a few thousand cover the edge cases. Spend effort on coverage, not count.

### Deduplicate

Near-duplicates overweight one pattern and burn epochs repeating it. Dedup on content, not just exact string match.

### Decontaminate

Drop anything that overlaps your eval split. A leaked eval prompt inflates the score, and the number stops meaning anything.

## Slide 30

**03 How models learn**

# How OPD works.

The student writes; the teacher scores every token of that exact attempt; training pulls the student toward the teacher.

> min KL( π_student ‖ π_teacher )

Reverse KL, computed per token on the student's own samples.

### 01 · Student attempts

Sample a completion from the current student for each prompt.

### 02 · Teacher grades

The teacher computes its own distribution over every token the student produced.

### 03 · Pull token by token

Minimize reverse KL so the student's distribution moves toward the teacher's at each position.

### 04 · Warm-start from SFT

`init_from_adapter` starts from a format-competent student; a cold OPD run tends to underperform SFT.

## Slide 31

**03 How models learn**

# How GRPO works.

RL with a batch statistic instead of a value network. "Did this rollout beat its siblings?" is the entire signal.

> Aᵢ = ( rᵢ − μ ) / σ

The advantage: μ and σ are the mean and spread of the group's rewards.

### 01 · Sample a group

Draw `group_size` rollouts per prompt from the current model.

### 02 · Score each

Your environment's reward gives every rollout a number.

### 03 · Compute advantage

Normalize each rollout's score against its own group's mean and spread.

### 04 · Update on a leash

Raise token probabilities in above-average rollouts, held near the reference by the β·KL penalty.

## Slide 32

**03 How models learn**

# GRPO failure modes.

The first run rarely fails loudly: a flat reward curve, rollouts that all look alike. Four patterns cover most of it.

> no spread → no advantage → no gradient

When rollouts stop differing (σ = 0), the group signal dies; three of the four failures end here.

### 01 · Truncation

`max_completion_tokens` too low: every rollout truncates, every reward is zero. Raise the cap first.

### 02 · Length hacking

Reward correlates with verbosity and answers balloon. Cap length in the reward or score concision.

### 03 · Entropy collapse

Rollouts converge to near-identical text and learning stalls. Raise temperature or diversify prompts.

### 04 · Format collapse

One high-scoring template gets repeated everywhere. Patch the reward's gap; check the eval for variety.

## Slide 33

**03 How models learn**

# SFT & OPD failure modes.

The supervised half fails more quietly than GRPO. Two patterns per algorithm cover most bad runs.

> training loss ↓ ≠ eval ↑

The gap between those two curves is where every failure below hides.

### 01 · Overfitting · SFT

Too many epochs on too few rows: loss keeps falling, the held-out eval sinks. Cut epochs, add coverage.

### 02 · Memorized eval · SFT

Eval prompts leaked into training. The score inflates and means nothing. Decontaminate first.

### 03 · Cold start · OPD

A student that can't produce the format gives the teacher nothing to grade. Warm-start from SFT.

### 04 · Teacher mismatch · OPD

The teacher isn't actually better at your task, so the student inherits its mistakes. Eval the teacher first.

## Slide 34

**03 How models learn**

# Reading the curves.

Three lines tell the story of a run: reward, held-out eval, entropy. Their shapes diagnose a run while it's still training.

> reward ↑ · eval ↑ · entropy easing down

The healthy shape: all three move together and nothing falls off a cliff.

### 01 · Healthy

Reward climbs, the eval follows, entropy eases down. Keep going.

Small chart: reward and eval both trend upward and begin to level off.

### 02 · Stalled

Both flat: no gradient. Check truncation, group size, and whether any rollout succeeds.

Small chart: reward and eval remain noisy but flat.

### 03 · Hacked

Reward climbs while the eval doesn't. The proxy is being gamed: read traces, patch the reward.

Small chart: reward rises and levels off while eval stays near the baseline.

### 04 · Entropy crash

Entropy plunges, rollouts converge, learning dies with the variance. Raise temperature or loosen KL.

Small chart: entropy falls steeply to a low plateau while reward rises only modestly.

Chart legend: `reward` in blue, `eval` in dark navy, and `entropy` in gray.

## Slide 35

**03 How models learn**

# Multi-turn agents.

Agent tasks are episodes: tool calls, observations, an outcome. The question is where the reward lands: episode, or turn?

### Episode reward · sparse

**One score at the end.**

Did the ticket resolve, did the tests pass. Easy to define, but one number spread over twelve turns is thin credit.

### Turn-level reward · dense

**Score the steps.**

Grade tool calls and decisions as they happen. Denser signal, sharper credit, and a reward you design per step.

## Slide 36

**03 How models learn**

# Catastrophic forgetting.

Tuning hard on one task can sand away general ability. LoRA limits the blast radius; it doesn't eliminate drift.

After every tune, run two evals: your task's, and a general one.

### Task eval

Your environment's held-out split: the number you trained to move. This one should climb.

### General eval

A broad capability check, like instruction following or a general benchmark slice. It should hold roughly flat, run over run.

### If it drifts

Lower epochs or learning rate, mix general data back in, or drop the LoRA rank. Catch it at the checkpoint, not in production.

## Slide 37

**Section 04**

# 04

# Flash.

Layout: dark navy section divider with blue, green, and black pixel blocks on the right.

## Slide 38

**04 Flash**

# What Freesolo does.

Managed post-training, driven by your coding agent. One config, one command; a deployable model comes back. The weights are yours.

### 01 · Describe the run

Name the base model, algorithm, and environment; point at your data.

### 02 · Approve the run

Flash validates the config and returns an ETA before anything starts. Nothing runs until you say go.

### 03 · Own the model

A trained LoRA adapter comes back. Deploy it, or export the weights to your own repo.

```toml
model = "Qwen/Qwen3.5-4B"
algorithm = "grpo"                # or "sft" / "opd"
[environment]
id = "your-org/your-env"
```

## Slide 39

**04 Flash**

# The post-training stack.

Agents at the top, compute at the bottom. You write exactly one layer: the environment. Flash owns everything below it.

The stack runs top to bottom:

| Layer | Detail |
| --- | --- |
| **Your agent** | Claude Code or Cursor drives the CLI: `flash train` → `flash deploy` → `flash chat`. |
| **Algorithms** | SFT · GRPO · OPD, switched with one line of config. |
| **Environment** | Your task + reward, published to the Hub by id. **YOU WRITE THIS** |
| **Base models** | Qwen3.5 and friends. Browse the catalog with `flash models`. |
| **Managed compute** | GPUs, checkpointing, retries, and artifact storage, all managed for you. |

The environment row is blue with a green `YOU WRITE THIS` badge. The managed compute row is dark navy.

## Slide 40

**04 Flash**

# Mixture of experts.

Instead of one dense network, many expert sub-networks and a router that picks a few per token. Total size and compute decouple.

> 35B parameters · ~3B active per token

The catalog's Qwen3.6-35B-A3B: A3B means about three billion active parameters.

### 01 · The router

A small gate scores every expert per token and sends the token to the top few.

### 02 · Sparse compute

Only chosen experts run, so a 35B model serves near the cost of a 3B dense one.

### 03 · Why it wins

More capacity at fixed compute: knowledge lives across experts, latency stays small-model.

### 04 · Tuning a MoE

LoRA works the same: adapters wrap attention as usual; the router stays frozen.

## Slide 41

**04 Flash**

# Pick a base model.

**Every model: SFT · GRPO · OPD · thinking**

| Model | Size | Context | Max LoRA rank |
| --- | ---: | ---: | ---: |
| `Qwen/Qwen3.5-0.8B` | 0.8B | 8,192 | 128 |
| `openbmb/MiniCPM5-1B` | 1B | 8,192 | 128 |
| `Qwen/Qwen3.5-2B` | 2B | 8,192 | 128 |
| `Qwen/Qwen3.5-4B` | 4B | 8,192 | 64 |
| `Qwen/Qwen3.5-9B` | 9B | 8,192 | 64 |
| `Qwen/Qwen3.6-35B-A3B` | 35B MoE | 4,096 | 64 |

## Slide 42

**04 Flash**

# One loop, three algorithms.

1. **01 · Attempt**

   The current model answers a prompt from your environment.

2. **02 · Score**

   The environment grades it: gold answer, reward, or teacher.

3. **03 · Update**

   SFT, GRPO, or OPD nudges the weights toward higher scores.

4. **04 · Repeat**

   Until the eval says done. The output is a LoRA adapter.

The four cards are connected left to right by arrows.

```console
$ flash train config.toml     # the whole loop is one command: algorithm = "sft" | "grpo" | "opd"
```

## Slide 43

**04 Flash**

# Hyperparameters.

The `[train]` block has a dozen knobs, and four of them decide most runs. The defaults are sane; drift from them one knob at a time.

### Touch these first

**The four that move results.**

`learning_rate`, `epochs`, `group_size`, `max_completion_tokens`: speed, duration, GRPO signal, and rollout headroom.

- **+** `learning_rate` and `epochs` drive SFT; `group_size` and `max_completion_tokens` drive GRPO.
- **−** Change one per run so you know which worked.

### Leave these alone

**Defaults are fine until a trace objects.**

`batch_size`, `warmup`, KL β, LoRA dropout, the learning-rate schedule. All shipped with defaults that survive most runs.

- **+** Tuned per base model; they rarely gate results.
- **−** Touch them only when a trace names the problem.

## Slide 44

**04 Flash**

# Structured outputs.

Guided decoding doesn't ask for a format. It makes anything else impossible: format-breaking tokens are masked out.

> p( format-breaking token ) = 0

The decoder re-masks the vocabulary at every generation step.

### 01 · The constraint

A JSON schema, a regex, or a fixed choice set; the decoder samples only tokens that keep the output valid.

### 02 · From token one

The mask binds from the first generated token, so it pairs with non-thinking mode; thinking tokens would break the schema.

### 03 · During training

Constrained GRPO and OPD rollouts can't make format errors, so the reward grades content instead of punctuation.

### 04 · At serve time

The deployed endpoint keeps the constraint: parseable output on every call, no retries, no JSON repair.

## Slide 45

**04 Flash**

# Serving the trained model.

### Managed serving · default

**One command, OpenAI-compatible.**

```console
$ flash deploy <run-id>
$ flash chat <run-id> -m "Hi"
# or any OpenAI SDK: model = <run-id>
```

Prefix caching is always on, and a deployed adapter keeps the thinking mode it was trained with.

Point any OpenAI client at the endpoint from `flash deployments`; deploy specific checkpoints with `<run-id>/step-N`.

### Or export & serve anywhere

```console
$ flash export --adapter-id <run-id> --repository you/model
```

| Provider | Description |
| --- | --- |
| Parasail | On-demand GPU network for dedicated, low-cost inference endpoints. |
| Baseten | Production inference platform with autoscaling dedicated deployments. |
| Together AI | Serverless and dedicated endpoints across open models, LoRA-aware. |
| Fireworks AI | Fast serverless inference; multi-LoRA serving on shared base models. |
| Modal | Serverless Python compute. Run your own vLLM server, full control. |

## Slide 46

**Training deep dive**

# Specialization

# beats the frontier.

**START TRAINING**

Layout: dark navy closing slide with a green rounded `START TRAINING` button and interlocking blue, green, and black pixel blocks on the right.
