"""Single source of truth for Quip dataset policy and validation."""

from __future__ import annotations

import hashlib
import json
import re
import unicodedata
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from better_profanity import profanity
from scoring import score_completion


ROOT = Path(__file__).resolve().parents[1]
DATASET_DIR = ROOT / "dataset"
CACHE_DIR = ROOT / ".data-cache"
SOURCE_CACHE_DIR = CACHE_DIR / "sources"
MANIFEST_PATH = DATASET_DIR / "source_manifest.json"
REPORT_PATH = DATASET_DIR / "build_report.json"


@dataclass(frozen=True)
class DatasetContract:
    train_size: int = 2000
    eval_size: int = 200
    test_size: int = 200
    window_sizes: tuple[int, ...] = (1, 2, 3, 4, 5)
    unchanged_share: float = 0.10
    augmentation_events: int = 1
    minimum_zipf_frequency: float = 3.0

    def expected_counts(self, split: str) -> dict[str, Any]:
        rows = {
            "train": self.train_size,
            "eval": self.eval_size,
            "test": self.test_size,
        }.get(split)
        if rows is None:
            raise ValueError(f"unknown dataset split: {split}")
        if rows % len(self.window_sizes):
            raise ValueError(f"{split} size must divide evenly across window sizes")
        per_size = rows // len(self.window_sizes)
        unchanged_per_size = int(per_size * self.unchanged_share)
        if unchanged_per_size / per_size != self.unchanged_share:
            raise ValueError(f"{split} unchanged share must produce exact integer quotas")
        return {
            "rows": rows,
            "unchanged": unchanged_per_size * len(self.window_sizes),
            "changed": (per_size - unchanged_per_size) * len(self.window_sizes),
            "window_sizes": {size: per_size for size in self.window_sizes},
            "unchanged_by_size": {
                size: unchanged_per_size for size in self.window_sizes
            },
        }


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
ENGLISH_TOKEN_RE = re.compile(r"[A-Za-z]+(?:'[A-Za-z]+)?")

ALLOWED_ROW_KEYS = {"input", "output", "metadata"}
REQUIRED_METADATA_KEYS = {
    "example_id",
    "family_id",
    "source_dataset",
    "source_record_id",
    "source_partition",
    "source_license",
    "generation",
    "category",
    "target_changed",
    "accepted_suggestions",
    "window_size",
}


@dataclass(frozen=True)
class Candidate:
    text: str
    target: str
    source_dataset: str
    source_record_id: str
    source_partition: str
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


def source_rejection_reason(value: str, *, check_profanity: bool = True) -> str | None:
    if not isinstance(value, str) or not value.strip():
        return "empty"
    if "\n" in value or "\r" in value:
        return "multiline"
    value = re.sub(r"\s+", " ", value).strip()
    if not WORDLIKE_RE.search(value):
        return "not_text"
    if check_profanity and profanity.contains_profanity(value):
        return "profanity"
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
    return None


def source_quality_rejection_reason(value: str) -> str | None:
    from wordfreq import zipf_frequency

    tokens = ENGLISH_TOKEN_RE.findall(value)
    if not tokens:
        return "no_english_tokens"
    if any(
        zipf_frequency(token.casefold(), "en") < CONTRACT.minimum_zipf_frequency
        for token in tokens
    ):
        return "rare_or_misspelled_token"
    return None


def window_rejection_reason(
    value: str,
    *,
    expected_size: int,
    check_profanity: bool = True,
) -> str | None:
    reason = source_rejection_reason(value, check_profanity=check_profanity)
    if reason:
        return reason
    if word_count(value) != expected_size:
        return "window_size"
    if expected_size not in CONTRACT.window_sizes:
        return "unsupported_window_size"
    if len(value) > 160:
        return "too_long"
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
        "input": {"text": candidate.text},
        "output": output_payload,
        "metadata": {
            "example_id": example_id,
            "family_id": candidate.family_id,
            "source_dataset": candidate.source_dataset,
            "source_record_id": candidate.source_record_id,
            "source_partition": candidate.source_partition,
            "source_license": candidate.source_license,
            "generation": generation,
            "category": candidate.category,
            "target_changed": candidate.target_changed,
            "accepted_suggestions": [suggestion],
            "window_size": word_count(candidate.text),
        },
    }


def row_text(row: dict[str, Any], path: Path, line_number: int) -> str:
    payload = row["input"]
    if not isinstance(payload, dict) or set(payload) != {"text"} or not isinstance(payload["text"], str):
        raise ValueError(f"{path}:{line_number}: input must contain only string field text")
    return payload["text"]


def row_suggestion(row: dict[str, Any], path: Path, line_number: int) -> str:
    payload = row["output"]
    if not isinstance(payload, dict) or set(payload) != {"suggestion"} or not isinstance(payload["suggestion"], str):
        raise ValueError(f"{path}:{line_number}: output must contain only string field suggestion")
    return payload["suggestion"]


def validate_generation_metadata(generation: Any, path: Path, line_number: int) -> None:
    if not isinstance(generation, dict):
        raise ValueError(f"{path}:{line_number}: generation metadata is invalid")
    method = generation.get("method")
    if method == "sourced":
        valid = set(generation) == {"method"}
    elif method == "qwerty_augmentation":
        operations = generation.get("operations")
        valid = (
            set(generation) == {"method", "seed", "requested_events", "operations"}
            and isinstance(generation["seed"], int)
            and not isinstance(generation["seed"], bool)
            and generation["requested_events"] == CONTRACT.augmentation_events
            and isinstance(operations, list)
            and len(operations) == CONTRACT.augmentation_events
            and all(
                isinstance(operation, dict)
                and set(operation) == {"event", "operator", "index", "source", "replacement"}
                and isinstance(operation["event"], int)
                and isinstance(operation["operator"], str)
                and isinstance(operation["index"], int)
                and isinstance(operation["source"], str)
                and isinstance(operation["replacement"], str)
                for operation in operations
            )
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
            if not isinstance(row["input"], dict) or not isinstance(row["output"], dict):
                raise ValueError(f"{path}:{line_number}: input and output must be objects")
            metadata = row["metadata"]
            if not isinstance(metadata, dict) or not REQUIRED_METADATA_KEYS.issubset(metadata):
                raise ValueError(f"{path}:{line_number}: metadata provenance fields are incomplete")
            text = row_text(row, path, line_number)
            suggestion = row_suggestion(row, path, line_number)
            size = metadata["window_size"]
            if not isinstance(size, int) or isinstance(size, bool):
                raise ValueError(f"{path}:{line_number}: window_size must be an integer")
            for value in (text, suggestion):
                reason = window_rejection_reason(value, expected_size=size)
                if reason:
                    raise ValueError(f"{path}:{line_number}: rejected text: {reason}")
            if not isinstance(metadata["target_changed"], bool):
                raise ValueError(f"{path}:{line_number}: target_changed must be boolean")
            if metadata["target_changed"] != (suggestion != text):
                raise ValueError(f"{path}:{line_number}: target_changed is incorrect")
            accepted = metadata["accepted_suggestions"]
            if not isinstance(accepted, list) or not accepted or not all(
                isinstance(value, str) and value.strip() for value in accepted
            ):
                raise ValueError(f"{path}:{line_number}: accepted_suggestions is invalid")
            if normalize_text(suggestion) not in {normalize_text(value) for value in accepted}:
                raise ValueError(f"{path}:{line_number}: gold suggestion is not accepted")
            for key in (
                "example_id",
                "family_id",
                "source_dataset",
                "source_record_id",
                "source_partition",
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
                response_text=suggestion,
            )
            if result.score != 1.0 or not result.success:
                raise ValueError(f"{path}:{line_number}: gold output failed reward: {result.reason}")
            rows.append(row)
    if not rows:
        raise ValueError(f"{path}: dataset is empty")
    return rows


def validate_split(name: str, rows: list[dict[str, Any]]) -> None:
    expected = CONTRACT.expected_counts(name)
    changes = Counter(row["metadata"]["target_changed"] for row in rows)
    sizes = Counter(row["metadata"]["window_size"] for row in rows)
    unchanged_by_size = Counter(
        row["metadata"]["window_size"]
        for row in rows
        if not row["metadata"]["target_changed"]
    )
    actual = {
        "rows": len(rows),
        "unchanged": changes[False],
        "changed": changes[True],
        "window_sizes": dict(sorted(sizes.items())),
        "unchanged_by_size": dict(sorted(unchanged_by_size.items())),
    }
    if actual != expected:
        raise ValueError(f"{name}: quota mismatch: expected {expected}, got {actual}")
    inputs = [normalize_text(row_text(row, Path(name), index)) for index, row in enumerate(rows, 1)]
    if len(inputs) != len(set(inputs)):
        raise ValueError(f"{name}: normalized duplicate inputs exist")


def validate_compiled_datasets(
    *,
    train_path: Path,
    eval_path: Path,
    test_path: Path,
    report_path: Path,
) -> None:
    train_rows = load_rows(train_path)
    eval_rows = load_rows(eval_path)
    test_rows = load_rows(test_path)
    validate_split("train", train_rows)
    validate_split("eval", eval_rows)
    validate_split("test", test_rows)

    all_rows = train_rows + eval_rows + test_rows
    ids = [row["metadata"]["example_id"] for row in all_rows]
    if len(set(ids)) != len(ids):
        raise ValueError("metadata.example_id values must be globally unique")
    split_rows = {"train": train_rows, "eval": eval_rows, "test": test_rows}
    expected_partitions = {"train": {"train"}, "eval": {"dev"}, "test": {"test"}}
    family_sets: dict[str, set[str]] = {}
    for name, rows in split_rows.items():
        if {row["metadata"]["source_partition"] for row in rows} != expected_partitions[name]:
            raise ValueError(f"{name} rows come from the wrong MASSIVE partition")
        family_sets[name] = {row["metadata"]["family_id"] for row in rows}
    split_names = tuple(split_rows)
    for index, left in enumerate(split_names):
        for right in split_names[index + 1 :]:
            if family_sets[left] & family_sets[right]:
                raise ValueError(f"{left} and {right} contain source-family leakage")

    if not report_path.is_file():
        raise ValueError("dataset build report is missing")
    report = json.loads(report_path.read_text(encoding="utf-8"))
    if report.get("dataset_policy") != "massive_window_augmentation_v1":
        raise ValueError("dataset build report policy is incorrect")
    for name, path in (("train", train_path), ("eval", eval_path), ("test", test_path)):
        split_report = report.get("splits", {}).get(name, {})
        if split_report.get("sha256") != sha256_file(path):
            raise ValueError(f"{name}: dataset hash does not match build report")
        if split_report.get("rows") != CONTRACT.expected_counts(name)["rows"]:
            raise ValueError(f"{name}: build report row count is incorrect")
    if report.get("source_manifest_sha256") != sha256_file(MANIFEST_PATH):
        raise ValueError("source manifest hash does not match build report")

    for name, rows in (("train", train_rows), ("eval", eval_rows), ("test", test_rows)):
        categories = Counter(row["metadata"]["category"] for row in rows)
        sizes = Counter(row["metadata"]["window_size"] for row in rows)
        print(
            f"{name}: {len(rows)} rows "
            f"(categories={dict(sorted(categories.items()))}, windows={dict(sorted(sizes.items()))})"
        )
