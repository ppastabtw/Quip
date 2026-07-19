# Quip judge-backed GRPO environment

This directory is an isolated Freesolo environment overlay for a warm-started
GRPO run. Before publication, stage this directory with the finalized dataset,
 the root `environment.py`, `lexical_candidates.py`, `scoring.py`, system
 prompts, `pyproject.toml`, and `uv.lock`. The staged `environment.py` remains
 the environment entrypoint.

The reward uses deterministic schema and change-decision gates first. Exact
accepted suggestions receive full reward without a network call. Other
schema-valid candidates with the correct change decision are graded by
Freesolo-managed `Qwen/Qwen3.6-35B-A3B` serving. Judge failures fall back to
the deterministic partial reward and do not fail the training worker.

Flash injects its platform-managed `FREESOLO_API_KEY` into the worker. The
config does not declare or transmit a user-supplied judge secret.
