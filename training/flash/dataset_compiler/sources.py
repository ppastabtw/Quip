"""Download and window the single MASSIVE en-US source."""

from __future__ import annotations

import json
import shutil
import tarfile
import urllib.request
import random
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Sequence

from .contract import (
    CONTRACT,
    MANIFEST_PATH,
    SOURCE_CACHE_DIR,
    BuildError,
    Candidate,
    normalize_text,
    sha256_file,
    source_quality_rejection_reason,
    source_rejection_reason,
    window_rejection_reason,
)


def load_manifest() -> dict[str, Any]:
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 2:
        raise BuildError("source manifest schema_version must be 2")
    if set(manifest.get("sources", {})) != {"massive_en_us"}:
        raise BuildError("source manifest must contain only massive_en_us")
    return manifest


def _download(url: str, destination: Path) -> None:
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as handle:
        shutil.copyfileobj(response, handle)
    temporary.replace(destination)


def prepare_sources(manifest: dict[str, Any], *, offline: bool) -> dict[str, Path]:
    SOURCE_CACHE_DIR.mkdir(parents=True, exist_ok=True)
    source = manifest["sources"]["massive_en_us"]
    archive_path = SOURCE_CACHE_DIR / source["filename"]
    if not archive_path.is_file():
        legacy_cache = SOURCE_CACHE_DIR.parent / source["filename"]
        if legacy_cache.is_file():
            shutil.copyfile(legacy_cache, archive_path)
        elif offline:
            raise BuildError(f"offline source is missing: {archive_path}")
        else:
            _download(source["url"], archive_path)
    if sha256_file(archive_path) != source["sha256"]:
        raise BuildError(f"source checksum mismatch: {archive_path.name}")

    member_name = source["archive_member"]
    extracted_path = SOURCE_CACHE_DIR / "massive-en-US.jsonl"
    if not extracted_path.is_file():
        with tarfile.open(archive_path, "r:gz") as archive:
            member = archive.getmember(member_name)
            if not member.isfile():
                raise BuildError(f"MASSIVE archive member is not a file: {member_name}")
            source_handle = archive.extractfile(member)
            if source_handle is None:
                raise BuildError(f"MASSIVE archive member is unreadable: {member_name}")
            temporary = extracted_path.with_suffix(".jsonl.tmp")
            with source_handle, temporary.open("wb") as destination:
                shutil.copyfileobj(source_handle, destination)
            temporary.replace(extracted_path)
    if sha256_file(extracted_path) != source["member_sha256"]:
        raise BuildError("MASSIVE en-US member checksum mismatch")
    return {"massive_en_us": extracted_path}


def parse_massive(path: Path) -> list[dict[str, str]]:
    records: list[dict[str, str]] = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            row = json.loads(line)
            required = {"id", "locale", "partition", "scenario", "intent", "utt"}
            if not isinstance(row, dict) or not required.issubset(row):
                raise BuildError(f"invalid MASSIVE row at line {line_number}")
            if row["locale"] != "en-US" or row["partition"] not in {"train", "dev", "test"}:
                raise BuildError(f"invalid MASSIVE locale or partition at line {line_number}")
            if not all(isinstance(row[key], str) and row[key] for key in required):
                raise BuildError(f"invalid MASSIVE string field at line {line_number}")
            records.append({key: row[key] for key in required})
    if not records:
        raise BuildError("MASSIVE parser produced no en-US records")
    return records


def sample_massive_windows(
    records: Sequence[dict[str, str]],
    *,
    partition: str,
    required_by_size: dict[int, int],
    rng: random.Random,
    buffer_rows: int | Mapping[int, int] | None = None,
) -> dict[int, list[Candidate]]:
    windows: dict[int, list[Candidate]] = {size: [] for size in CONTRACT.window_sizes}
    seen: dict[int, set[str]] = {size: set() for size in CONTRACT.window_sizes}
    pool_targets = {
        size: required_by_size[size]
        + (
            max(10, required_by_size[size] // 5)
            if buffer_rows is None
            else buffer_rows[size]
            if isinstance(buffer_rows, Mapping)
            else buffer_rows
        )
        for size in CONTRACT.window_sizes
    }
    candidates = [record for record in records if record["partition"] == partition]
    rng.shuffle(candidates)

    for record in candidates:
        if all(len(windows[size]) >= pool_targets[size] for size in CONTRACT.window_sizes):
            break
        utterance = " ".join(record["utt"].split())
        if source_rejection_reason(utterance) or source_quality_rejection_reason(utterance):
            continue
        tokens = utterance.split()
        eligible_sizes = [
            size
            for size in CONTRACT.window_sizes
            if len(tokens) >= size and len(windows[size]) < pool_targets[size]
        ]
        if not eligible_sizes:
            continue
        rng.shuffle(eligible_sizes)
        size = min(
            eligible_sizes,
            key=lambda value: len(windows[value]) / pool_targets[value],
        )
        start = rng.randrange(0, len(tokens) - size + 1)
        text = " ".join(tokens[start : start + size])
        if window_rejection_reason(
            text,
            expected_size=size,
            check_profanity=False,
        ):
            continue
        normalized = normalize_text(text)
        if normalized in seen[size]:
            continue
        seen[size].add(normalized)
        family_id = f"massive_en_us:{partition}:{record['id']}"
        windows[size].append(
            Candidate(
                text=text,
                target=text,
                source_dataset="massive_en_us_1_1",
                source_record_id=f"en-US:{partition}:{record['id']}:w{size}:s{start}",
                source_partition=partition,
                source_license="CC BY 4.0",
                family_id=family_id,
                category="natural_keep",
                target_changed=False,
            )
        )

    missing = {
        size: (len(windows[size]), pool_targets[size])
        for size in CONTRACT.window_sizes
        if len(windows[size]) < pool_targets[size]
    }
    if missing:
        raise BuildError(f"MASSIVE sparse sampling did not fill window pools: {missing}")
    return windows
