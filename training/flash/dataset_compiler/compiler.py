"""Deterministic split construction and teacher augmentation orchestration."""

from __future__ import annotations

import json
import random
from collections import Counter
from pathlib import Path
from typing import Any, Sequence

import httpx

from .backboard import (
    GENERATION_MODEL,
    GENERATION_SYSTEM_PROMPT,
    VERIFICATION_MODEL,
    VERIFICATION_SYSTEM_PROMPT,
    BackboardClient,
    validate_generation_payload,
    validate_verification_payload,
)
from .contract import (
    CONTRACT,
    DATASET_DIR,
    GENERATION_SCHEMA_PATH,
    MANIFEST_PATH,
    REPORT_PATH,
    VERIFICATION_SCHEMA_PATH,
    BuildError,
    Candidate,
    compact_json,
    is_near_duplicate,
    make_row,
    normalize_text,
    pair_rejection_reason,
    sha256_file,
    validate_compiled_datasets,
)
from .sources import (
    choose_candidates,
    jfleg_candidates,
    load_manifest,
    multilexnorm_candidates,
    parse_jfleg,
    parse_multilexnorm,
    parse_uci_ham,
    prepare_sources,
    uci_candidates,
)


def augment_targets(
    targets: Sequence[Candidate],
    client: BackboardClient,
    *,
    blocked_texts: Sequence[str] = (),
    required_count: int | None = None,
) -> list[Candidate]:
    required = len(targets) if required_count is None else required_count
    if required < 1 or required > len(targets):
        raise BuildError("teacher required_count is outside the target pool")
    blocked_normalized = {normalize_text(value) for value in blocked_texts}
    accepted: dict[str, Candidate] = {}

    def run_round(round_targets: Sequence[Candidate], attempt: int) -> list[Candidate]:
        rejected_round: list[Candidate] = []
        for batch_start in range(0, len(round_targets), CONTRACT.batch_size):
            batch = list(round_targets[batch_start : batch_start + CONTRACT.batch_size])
            target_ids = [item.source_record_id for item in batch]
            generation = client.complete(
                system_prompt=GENERATION_SYSTEM_PROMPT,
                user_payload={
                    "attempt": attempt,
                    "targets": [
                        {"target_id": item.source_record_id, "target": item.target}
                        for item in batch
                    ],
                },
                phase="generation",
            )
            drafts = validate_generation_payload(generation, target_ids)
            pairs = [
                {
                    "target_id": item.source_record_id,
                    "draft": drafts[item.source_record_id],
                    "target": item.target,
                }
                for item in batch
            ]
            verification = client.complete(
                system_prompt=VERIFICATION_SYSTEM_PROMPT,
                user_payload={"pairs": pairs},
                phase="verification",
            )
            verdicts = validate_verification_payload(verification, target_ids)
            batch_normalized: set[str] = set()
            for item in batch:
                draft = drafts[item.source_record_id]
                verdict = verdicts[item.source_record_id]
                reason = pair_rejection_reason(draft, item.target, teacher=True)
                normalized_draft = normalize_text(draft)
                if (
                    reason
                    or not verdict["accepted"]
                    or normalized_draft in blocked_normalized
                    or normalized_draft in batch_normalized
                ):
                    rejected_round.append(item)
                    continue
                batch_normalized.add(normalized_draft)
                accepted[item.source_record_id] = Candidate(
                    text=draft,
                    target=item.target,
                    source_dataset=item.source_dataset,
                    source_record_id=item.source_record_id,
                    source_license=item.source_license,
                    family_id=item.family_id,
                    category="teacher_shorthand",
                    target_changed=True,
                )
            blocked_normalized.update(batch_normalized)
        return rejected_round

    rejected = run_round(targets, 1)
    remaining_calls = CONTRACT.max_teacher_requests - client.request_count
    retry_batches = max(0, remaining_calls // 2)
    retry_targets = rejected[: retry_batches * CONTRACT.batch_size]
    if retry_targets:
        run_round(retry_targets, 2)
    ordered = [
        accepted[item.source_record_id]
        for item in targets
        if item.source_record_id in accepted
    ]
    if len(ordered) < required:
        raise BuildError(
            f"teacher accepted {len(ordered)} of {required} required rows after one regeneration"
        )
    return ordered[:required]


def write_jsonl(path: Path, rows: Sequence[dict[str, Any]]) -> None:
    rendered = "".join(compact_json(row) + "\n" for row in rows)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(rendered, encoding="utf-8", newline="\n")
    temporary.replace(path)


def split_summary(rows: Sequence[dict[str, Any]], path: Path) -> dict[str, Any]:
    changes = Counter(row["metadata"]["target_changed"] for row in rows)
    return {
        "rows": len(rows),
        "sha256": sha256_file(path),
        "changes": {
            "unchanged": changes[False],
            "changed": changes[True],
        },
        "single_word": sum(row["metadata"]["is_single_word"] for row in rows),
        "single_word_keep": sum(
            row["metadata"]["is_single_word"]
            and not row["metadata"]["target_changed"]
            for row in rows
        ),
        "protected_rows": sum(bool(row["metadata"]["protected_tokens"]) for row in rows),
    }


def compile_datasets(
    *,
    seed: int,
    offline: bool,
    transport: httpx.BaseTransport | None = None,
) -> dict[str, Any]:
    manifest = load_manifest()
    sources = prepare_sources(manifest, offline=offline)
    json.loads(GENERATION_SCHEMA_PATH.read_text(encoding="utf-8"))
    json.loads(VERIFICATION_SCHEMA_PATH.read_text(encoding="utf-8"))
    rng = random.Random(seed)

    multi_train = parse_multilexnorm(sources["multilexnorm_en_train"])
    multi_test = parse_multilexnorm(sources["multilexnorm_en_test"])
    jfleg = parse_jfleg(
        sources["jfleg_test_source"],
        [sources[f"jfleg_test_ref{index}"] for index in range(4)],
    )
    uci = parse_uci_ham(sources["uci_sms_ham"])

    eval_keep = choose_candidates(
        multilexnorm_candidates(multi_test, split="test", action="keep"),
        count=150,
        single_count=27,
        rng=rng,
    )
    eval_replace_multi = choose_candidates(
        multilexnorm_candidates(multi_test, split="test", action="replace"),
        count=90,
        single_count=9,
        rng=rng,
        blocked_texts=[item.text for item in eval_keep],
    )
    eval_jfleg = choose_candidates(
        jfleg_candidates(jfleg),
        count=60,
        single_count=0,
        rng=rng,
        blocked_texts=[item.text for item in eval_keep + eval_replace_multi],
    )
    eval_candidates = eval_keep + eval_replace_multi + eval_jfleg
    eval_blocked = [item.text for item in eval_candidates]

    train_keep = choose_candidates(
        uci_candidates(uci, action="keep"),
        count=1000,
        single_count=180,
        rng=rng,
        blocked_texts=eval_blocked,
    )
    train_real_replace = choose_candidates(
        multilexnorm_candidates(multi_train, split="train", action="replace"),
        count=400,
        single_count=60,
        rng=rng,
        blocked_texts=eval_blocked + [item.text for item in train_keep],
    )
    teacher_targets = choose_candidates(
        [
            item
            for item in uci_candidates(uci, action="replace")
            if len(item.text.split()) > 1
        ],
        count=CONTRACT.teacher_target_pool_size,
        single_count=0,
        rng=rng,
        blocked_texts=eval_blocked
        + [item.text for item in train_keep + train_real_replace],
    )

    client = BackboardClient(offline=offline, transport=transport)
    try:
        train_teacher_replace = augment_targets(
            teacher_targets,
            client,
            blocked_texts=[item.text for item in train_keep + train_real_replace],
            required_count=600,
        )
        teacher_report = client.report()
    finally:
        client.close()

    if any(
        is_near_duplicate(item.text, blocked)
        for item in train_teacher_replace
        for blocked in eval_blocked
    ):
        raise BuildError("teacher augmentation introduced train and eval near-duplicate leakage")

    sourced_generation = {"method": "sourced", "teacher_model": None}
    teacher_generation = {
        "method": "backboard_augmentation",
        "teacher_model": GENERATION_MODEL,
        "verifier_model": VERIFICATION_MODEL,
    }
    train_rows = [
        make_row(
            item,
            generation=teacher_generation
            if item.category == "teacher_shorthand"
            else sourced_generation,
        )
        for item in train_keep + train_real_replace + train_teacher_replace
    ]
    eval_rows = [make_row(item, generation=sourced_generation) for item in eval_candidates]
    rng.shuffle(train_rows)
    rng.shuffle(eval_rows)

    DATASET_DIR.mkdir(parents=True, exist_ok=True)
    train_path = DATASET_DIR / "train.jsonl"
    eval_path = DATASET_DIR / "eval.jsonl"
    write_jsonl(train_path, train_rows)
    write_jsonl(eval_path, eval_rows)
    report = {
        "schema_version": 1,
        "corpus_policy": "research_only",
        "seed": seed,
        "constraints": {
            "train_size": CONTRACT.train_size,
            "eval_size": CONTRACT.eval_size,
            "change_balance": {"unchanged": 0.5, "changed": 0.5},
            "input_word_range": [1, CONTRACT.max_words],
            "single_word_share": 0.12,
            "single_word_keep_share": 0.75,
            "protected_token_max_share": CONTRACT.protected_token_max_share,
            "context": False,
            "personalization": False,
        },
        "source_manifest_sha256": sha256_file(MANIFEST_PATH),
        "teacher": teacher_report,
        "splits": {
            "train": split_summary(train_rows, train_path),
            "eval": split_summary(eval_rows, eval_path),
        },
    }
    REPORT_PATH.write_text(
        json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return report


def verify_only() -> None:
    validate_compiled_datasets(
        train_path=DATASET_DIR / "train.jsonl",
        eval_path=DATASET_DIR / "eval.jsonl",
        report_path=REPORT_PATH,
    )
