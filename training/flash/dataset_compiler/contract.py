"""Shared dataset contract, text rules, row construction, and validation."""

from __future__ import annotations

import hashlib
import json
import re
import unicodedata
from collections import Counter
from dataclasses import dataclass
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any

from scoring import score_completion


ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = ROOT.parents[1]
DATASET_DIR = ROOT / "dataset"
CACHE_DIR = ROOT / ".data-cache"
SOURCE_CACHE_DIR = CACHE_DIR / "sources"
TEACHER_CACHE_DIR = CACHE_DIR / "teacher"
MANIFEST_PATH = DATASET_DIR / "source_manifest.json"
REPORT_PATH = DATASET_DIR / "build_report.json"
GENERATION_SCHEMA_PATH = ROOT / "schemas" / "teacher_generation.schema.json"
VERIFICATION_SCHEMA_PATH = ROOT / "schemas" / "teacher_verification.schema.json"


@dataclass(frozen=True)
class DatasetContract:
    train_size: int = 2000
    eval_size: int = 300
    max_words: int = 5
    teacher_target_max_words: int = 10
    teacher_target_pool_size: int = 650
    batch_size: int = 20
    max_teacher_requests: int = 90
    protected_token_max_share: float = 0.05

    def expected_counts(self, split: str) -> dict[str, int]:
        if split == "train":
            return {
                "rows": self.train_size,
                "unchanged": 1000,
                "changed": 1000,
                "single": 240,
                "single_unchanged": 180,
            }
        if split == "eval":
            return {
                "rows": self.eval_size,
                "unchanged": 150,
                "changed": 150,
                "single": 36,
                "single_unchanged": 27,
            }
        raise ValueError(f"unknown dataset split: {split}")


CONTRACT = DatasetContract()

URL_RE = re.compile(r"(?:https?://|www\.|\b[a-z0-9.-]+\.(?:com|org|net|io)\b)", re.I)
EMAIL_RE = re.compile(r"\b[^\s@]+@[^\s@]+\.[^\s@]+\b")
PHONE_RE = re.compile(r"(?<!\w)(?:\+?\d[\s().-]*){7,}(?!\w)")
HANDLE_RE = re.compile(r"(?:^|\s)@[A-Za-z0-9_]+")
MARKUP_RE = re.compile(
    r"</?[A-Za-z][^>]*>|\[[^\]]+\]\([^)]*\)|```|\{[^{}]*:[^{}]*\}|&(?:lt|gt|amp|quot|#\d+);?",
    re.I,
)
CODE_RE = re.compile(r"(?:\b(?:def|class|import|return|const|let|var)\b|[{};]{2,}|\w+\([^)]*\)\s*[;{])")
WORDLIKE_RE = re.compile(r"[A-Za-z]")
PROTECTED_RE = re.compile(
    r"(?:[/\\]|\.[A-Za-z0-9]{1,5}\b|\b\w+_\w+\b|\bv?\d+(?:\.\d+){1,3}\b|\b[A-Fa-f0-9]{8,}\b)"
)
TEACHER_TARGET_RE = re.compile(r"^[A-Za-z][A-Za-z' ,.?!-]*$")
TEACHER_TARGET_SHORTHAND = {
    "b",
    "c",
    "coz",
    "d",
    "da",
    "dat",
    "dis",
    "im",
    "k",
    "lah",
    "leh",
    "lor",
    "msg",
    "n",
    "nite",
    "okie",
    "pls",
    "plz",
    "r",
    "txt",
    "u",
    "ur",
    "wat",
    "wen",
    "wif",
}

ALLOWED_ROW_KEYS = {"input", "output", "metadata"}
REQUIRED_METADATA_KEYS = {
    "example_id",
    "family_id",
    "source_dataset",
    "source_record_id",
    "source_license",
    "generation",
    "category",
    "target_changed",
    "accepted_suggestions",
    "word_count",
    "is_single_word",
    "protected_tokens",
}


@dataclass(frozen=True)
class Candidate:
    text: str
    target: str
    source_dataset: str
    source_record_id: str
    source_license: str
    family_id: str
    category: str
    target_changed: bool


class BuildError(RuntimeError):
    pass


def compact_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def normalize_text(value: str) -> str:
    value = unicodedata.normalize("NFKC", value)
    value = re.sub(r"\s+", " ", value).strip().casefold()
    return re.sub(r"[^\w']+", " ", value).strip()


def word_count(value: str) -> int:
    return len(value.split())


def is_near_duplicate(left: str, right: str) -> bool:
    a = normalize_text(left)
    b = normalize_text(right)
    if not a or not b:
        return False
    if a == b:
        return True
    a_words = a.split()
    b_words = b.split()
    if min(len(a_words), len(b_words)) < 3:
        return False
    if abs(len(a_words) - len(b_words)) > 1:
        return False
    return SequenceMatcher(None, a, b, autojunk=False).ratio() >= 0.94


def protected_tokens(value: str) -> list[str]:
    return [token for token in value.split() if PROTECTED_RE.search(token)]


def text_rejection_reason(
    value: str,
    *,
    allow_single: bool = True,
    max_words: int = CONTRACT.max_words,
) -> str | None:
    if not isinstance(value, str) or not value.strip():
        return "empty"
    if "\n" in value or "\r" in value:
        return "multiline"
    value = re.sub(r"\s+", " ", value).strip()
    count = word_count(value)
    if count < (1 if allow_single else 2) or count > max_words:
        return "word_count"
    if len(value) > 100 or not WORDLIKE_RE.search(value):
        return "not_plain_text"
    for name, pattern in (
        ("email", EMAIL_RE),
        ("url", URL_RE),
        ("phone", PHONE_RE),
        ("handle", HANDLE_RE),
        ("markup", MARKUP_RE),
        ("code", CODE_RE),
    ):
        if pattern.search(value):
            return name
    if protected_tokens(value):
        return "protected_token"
    return None


def pair_rejection_reason(source: str, target: str, *, teacher: bool = False) -> str | None:
    reason = text_rejection_reason(source, allow_single=not teacher)
    if reason:
        return reason
    target_reason = teacher_target_rejection_reason(target) if teacher else text_rejection_reason(target)
    if target_reason:
        return "invalid_target"
    if normalize_text(source) == normalize_text(target):
        return "unchanged"
    source_count = word_count(source)
    target_count = word_count(target)
    if target_count > max(source_count + 3, source_count * 2):
        return "excessive_rewrite"
    if not teacher and SequenceMatcher(
        None, normalize_text(source), normalize_text(target), autojunk=False
    ).ratio() < 0.35:
        return "semantic_rewrite"
    return None


def teacher_target_rejection_reason(value: str) -> str | None:
    reason = text_rejection_reason(
        value,
        allow_single=False,
        max_words=CONTRACT.teacher_target_max_words,
    )
    if reason:
        return reason
    if not TEACHER_TARGET_RE.fullmatch(value):
        return "teacher_target_characters"
    if value.isupper():
        return "teacher_target_all_caps"
    words = [word.strip(".!?").casefold() for word in value.split()]
    if any(word in TEACHER_TARGET_SHORTHAND for word in words):
        return "teacher_target_shorthand"
    if any(len(word) == 1 and word not in {"a", "i"} for word in words):
        return "teacher_target_fragment"
    return None


def uci_quality_rejection_reason(
    value: str,
    frequencies: Counter[str],
    *,
    single_keep: bool,
) -> str | None:
    if not TEACHER_TARGET_RE.fullmatch(value):
        return "uci_characters"
    words = [word.casefold() for word in re.findall(r"[A-Za-z']+", value)]
    if len(words) != word_count(value):
        return "uci_tokenization"
    if any(word in TEACHER_TARGET_SHORTHAND for word in words):
        return "uci_shorthand"
    if value.isupper() and value != "I":
        return "uci_all_caps"
    minimum_frequency = 10 if single_keep else 2
    if any(frequencies[word] < minimum_frequency for word in words):
        return "uci_rare_token"
    return None


def make_row(candidate: Candidate, *, generation: dict[str, Any]) -> dict[str, Any]:
    suggestion = candidate.target if candidate.target_changed else candidate.text
    output_payload = {"suggestion": suggestion}
    identity = {
        "source_dataset": candidate.source_dataset,
        "source_record_id": candidate.source_record_id,
        "generation": generation,
        "input": candidate.text,
        "output": output_payload,
    }
    example_id = "quip_" + sha256_bytes(compact_json(identity).encode("utf-8"))[:20]
    return {
        "input": compact_json({"text": candidate.text}),
        "output": compact_json(output_payload),
        "metadata": {
            "example_id": example_id,
            "family_id": candidate.family_id,
            "source_dataset": candidate.source_dataset,
            "source_record_id": candidate.source_record_id,
            "source_license": candidate.source_license,
            "generation": generation,
            "category": candidate.category,
            "target_changed": candidate.target_changed,
            "accepted_suggestions": [suggestion],
            "word_count": word_count(candidate.text),
            "is_single_word": word_count(candidate.text) == 1,
            "protected_tokens": protected_tokens(candidate.text),
        },
    }


def row_text(row: dict[str, Any], path: Path, line_number: int) -> str:
    try:
        payload = json.loads(row["input"])
    except json.JSONDecodeError as exc:
        raise ValueError(f"{path}:{line_number}: input is not valid JSON") from exc
    if not isinstance(payload, dict) or set(payload) != {"text"} or not isinstance(payload["text"], str):
        raise ValueError(f"{path}:{line_number}: input must contain only string field text")
    return payload["text"]


def validate_generation_metadata(generation: Any, path: Path, line_number: int) -> None:
    if not isinstance(generation, dict):
        raise ValueError(f"{path}:{line_number}: generation metadata is invalid")
    method = generation.get("method")
    if method == "sourced":
        valid = set(generation) == {"method", "teacher_model"} and generation["teacher_model"] is None
    elif method == "backboard_augmentation":
        valid = (
            set(generation) == {"method", "teacher_model", "verifier_model"}
            and isinstance(generation["teacher_model"], str)
            and bool(generation["teacher_model"])
            and isinstance(generation["verifier_model"], str)
            and bool(generation["verifier_model"])
        )
    else:
        valid = False
    if not valid:
        raise ValueError(f"{path}:{line_number}: generation metadata is invalid")


def load_rows(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_number}: invalid JSON: {exc}") from exc
            if not isinstance(row, dict) or set(row) != ALLOWED_ROW_KEYS:
                raise ValueError(f"{path}:{line_number}: row keys must be input, output, metadata")
            if not isinstance(row["input"], str) or not isinstance(row["output"], str):
                raise ValueError(f"{path}:{line_number}: input and output must be strings")
            metadata = row["metadata"]
            if not isinstance(metadata, dict) or not REQUIRED_METADATA_KEYS.issubset(metadata):
                raise ValueError(f"{path}:{line_number}: metadata provenance fields are incomplete")
            text = row_text(row, path, line_number)
            count = word_count(text)
            if not 1 <= count <= CONTRACT.max_words:
                raise ValueError(
                    f"{path}:{line_number}: input must contain 1 through {CONTRACT.max_words} words"
                )
            if metadata["word_count"] != count or metadata["is_single_word"] != (count == 1):
                raise ValueError(f"{path}:{line_number}: word-count metadata is incorrect")
            if not isinstance(metadata["protected_tokens"], list) or not all(
                isinstance(token, str) and token for token in metadata["protected_tokens"]
            ):
                raise ValueError(f"{path}:{line_number}: protected_tokens must be a string array")
            if not isinstance(metadata["target_changed"], bool):
                raise ValueError(f"{path}:{line_number}: target_changed must be boolean")
            if not isinstance(metadata["accepted_suggestions"], list) or not all(
                isinstance(suggestion, str) and suggestion.strip()
                for suggestion in metadata["accepted_suggestions"]
            ):
                raise ValueError(
                    f"{path}:{line_number}: accepted_suggestions must be a string array"
                )
            expected_suggestion = json.loads(row["output"])["suggestion"]
            if metadata["target_changed"] != (expected_suggestion != text):
                raise ValueError(f"{path}:{line_number}: target_changed is incorrect")
            if normalize_text(expected_suggestion) not in {
                normalize_text(suggestion)
                for suggestion in metadata["accepted_suggestions"]
            }:
                raise ValueError(
                    f"{path}:{line_number}: gold suggestion is not accepted"
                )
            for key in (
                "example_id",
                "family_id",
                "source_dataset",
                "source_record_id",
                "source_license",
                "category",
            ):
                if not isinstance(metadata[key], str) or not metadata[key]:
                    raise ValueError(f"{path}:{line_number}: metadata.{key} is required")
            validate_generation_metadata(metadata["generation"], path, line_number)
            result = score_completion(
                input_text=row["input"],
                expected_output=row["output"],
                metadata=metadata,
                response_text=row["output"],
            )
            if result.score != 1.0 or not result.success:
                raise ValueError(
                    f"{path}:{line_number}: gold output does not earn full reward: {result.reason}"
                )
            rows.append(row)
    if not rows:
        raise ValueError(f"{path}: dataset is empty")
    return rows


def validate_split(name: str, rows: list[dict[str, Any]]) -> None:
    expected = CONTRACT.expected_counts(name)
    changes = Counter(row["metadata"]["target_changed"] for row in rows)
    single_rows = [row for row in rows if row["metadata"]["is_single_word"]]
    single_unchanged = [
        row for row in single_rows if not row["metadata"]["target_changed"]
    ]
    actual = {
        "rows": len(rows),
        "unchanged": changes[False],
        "changed": changes[True],
        "single": len(single_rows),
        "single_unchanged": len(single_unchanged),
    }
    if actual != expected:
        raise ValueError(f"{name}: quota mismatch: expected {expected}, got {actual}")
    protected_share = sum(bool(row["metadata"]["protected_tokens"]) for row in rows) / len(rows)
    if protected_share > CONTRACT.protected_token_max_share:
        raise ValueError(f"{name}: protected-token share exceeds 5%")
    inputs = [row_text(row, Path(name), index) for index, row in enumerate(rows, 1)]
    normalized = [normalize_text(text) for text in inputs]
    if len(normalized) != len(set(normalized)):
        raise ValueError(f"{name}: exact or normalized duplicate inputs exist")


def validate_compiled_datasets(*, train_path: Path, eval_path: Path, report_path: Path) -> None:
    train_rows = load_rows(train_path)
    eval_rows = load_rows(eval_path)
    validate_split("train", train_rows)
    validate_split("eval", eval_rows)

    all_rows = train_rows + eval_rows
    ids = [row["metadata"]["example_id"] for row in all_rows]
    if len(set(ids)) != len(ids):
        raise ValueError("metadata.example_id values must be globally unique")

    train_families = {row["metadata"]["family_id"] for row in train_rows}
    eval_families = {row["metadata"]["family_id"] for row in eval_rows}
    if train_families & eval_families:
        raise ValueError("train and eval contain family leakage")
    train_repositories = {
        row["metadata"]["source_repository"]
        for row in train_rows
        if row["metadata"].get("source_repository")
    }
    eval_repositories = {
        row["metadata"]["source_repository"]
        for row in eval_rows
        if row["metadata"].get("source_repository")
    }
    if train_repositories & eval_repositories:
        raise ValueError("train and eval contain repository leakage")

    train_texts = [row_text(row, train_path, index) for index, row in enumerate(train_rows, 1)]
    eval_texts = [row_text(row, eval_path, index) for index, row in enumerate(eval_rows, 1)]
    for train_text in train_texts:
        for eval_text in eval_texts:
            if is_near_duplicate(train_text, eval_text):
                raise ValueError(
                    "train and eval contain normalized near-duplicate leakage: "
                    f"{train_text!r} and {eval_text!r}"
                )

    if not report_path.is_file():
        raise ValueError("dataset build report is missing")
    report = json.loads(report_path.read_text(encoding="utf-8"))
    if report.get("corpus_policy") != "research_only":
        raise ValueError("dataset build report must declare research_only")
    for name, path in (("train", train_path), ("eval", eval_path)):
        split_report = report.get("splits", {}).get(name, {})
        if split_report.get("sha256") != sha256_file(path):
            raise ValueError(f"{name}: dataset hash does not match build report")
        if split_report.get("rows") != CONTRACT.expected_counts(name)["rows"]:
            raise ValueError(f"{name}: build report row count is incorrect")

    manifest_path = train_path.parent / "source_manifest.json"
    if report.get("source_manifest_sha256") != sha256_file(manifest_path):
        raise ValueError("source manifest hash does not match build report")

    for name, rows in (("train", train_rows), ("eval", eval_rows)):
        counts = Counter(row["metadata"]["category"] for row in rows)
        summary = ", ".join(f"{category}={count}" for category, count in sorted(counts.items()))
        print(f"{name}: {len(rows)} rows ({summary})")
