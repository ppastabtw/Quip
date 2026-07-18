"""Pinned source acquisition, parsing, and candidate construction."""

from __future__ import annotations

import json
import random
import re
import zipfile
from collections import Counter
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any, Sequence

import httpx

from .contract import (
    CONTRACT,
    MANIFEST_PATH,
    SOURCE_CACHE_DIR,
    BuildError,
    Candidate,
    normalize_text,
    pair_rejection_reason,
    sha256_bytes,
    sha256_file,
    teacher_target_rejection_reason,
    text_rejection_reason,
    uci_quality_rejection_reason,
    word_count,
)


def load_manifest() -> dict[str, Any]:
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if manifest.get("corpus_policy") != "research_only":
        raise BuildError("source manifest must declare research_only")
    return manifest


def prepare_sources(manifest: dict[str, Any], *, offline: bool) -> dict[str, Path]:
    SOURCE_CACHE_DIR.mkdir(parents=True, exist_ok=True)
    paths: dict[str, Path] = {}
    with httpx.Client(follow_redirects=True, timeout=60.0) as client:
        for name, source in manifest["sources"].items():
            path = SOURCE_CACHE_DIR / source["filename"]
            expected = source["sha256"].lower()
            if path.is_file() and sha256_file(path) == expected:
                paths[name] = path
                continue
            if offline:
                raise BuildError(f"offline source cache is missing or invalid: {name}")
            response = client.get(source["url"])
            response.raise_for_status()
            actual = sha256_bytes(response.content)
            if actual != expected:
                raise BuildError(f"checksum mismatch for {name}: expected {expected}, got {actual}")
            path.write_bytes(response.content)
            paths[name] = path
    return paths


def parse_uci_ham(path: Path) -> list[tuple[str, str]]:
    records: list[tuple[str, str]] = []
    with zipfile.ZipFile(path) as archive:
        raw = archive.read("SMSSpamCollection").decode("utf-8")
    for index, line in enumerate(raw.splitlines()):
        label, separator, text = line.partition("\t")
        if separator and label == "ham":
            records.append((str(index), re.sub(r"\s+", " ", text).strip()))
    if len(records) < 4000:
        raise BuildError("UCI parser produced too few ham messages")
    return records


def parse_multilexnorm(path: Path) -> list[list[tuple[str, str]]]:
    sentences: list[list[tuple[str, str]]] = []
    current: list[tuple[str, str]] = []
    for line in path.read_text(encoding="utf-8").splitlines() + [""]:
        if not line.strip():
            if current:
                sentences.append(current)
                current = []
            continue
        source, separator, target = line.partition("\t")
        if not separator:
            raise BuildError(f"invalid MultiLexNorm line in {path.name}")
        current.append((source.strip(), target.strip()))
    if len(sentences) < 1000:
        raise BuildError(f"MultiLexNorm parser produced too few records from {path.name}")
    return sentences


def parse_jfleg(
    source_path: Path, reference_paths: Sequence[Path]
) -> list[tuple[int, int, str, str]]:
    sources = source_path.read_text(encoding="utf-8").splitlines()
    references = [path.read_text(encoding="utf-8").splitlines() for path in reference_paths]
    if any(len(reference) != len(sources) for reference in references):
        raise BuildError("JFLEG source and reference lengths differ")
    candidates: list[tuple[int, int, str, str]] = []
    for source_index, source in enumerate(sources):
        source_tokens = source.split()
        for reference_index, reference in enumerate(references):
            target_tokens = reference[source_index].split()
            matcher = SequenceMatcher(a=source_tokens, b=target_tokens, autojunk=False)
            for tag, left_start, left_end, right_start, right_end in matcher.get_opcodes():
                if tag == "equal" or left_start == left_end or right_start == right_end:
                    continue
                left = " ".join(source_tokens[left_start:left_end])
                right = " ".join(target_tokens[right_start:right_end])
                if word_count(left) < 2:
                    continue
                if pair_rejection_reason(left, right) is None:
                    candidates.append((source_index, reference_index, left, right))
    if len(candidates) < 60:
        raise BuildError("JFLEG parser produced too few short corrections")
    return candidates


def uci_candidates(records: Sequence[tuple[str, str]], *, action: str) -> list[Candidate]:
    result: list[Candidate] = []
    frequencies = Counter(
        word.casefold()
        for _, message in records
        for word in re.findall(r"[A-Za-z']+", message)
    )
    for record_id, message in records:
        items: list[tuple[str, str]] = []
        if action == "keep":
            items.extend((f"word:{index}", token) for index, token in enumerate(message.split()))
            phrase_values = [message] + re.split(r"[.!?,;:]+", message)
            items.extend(
                (f"phrase:{index}", phrase.strip(" \t\"'()[]{}-"))
                for index, phrase in enumerate(phrase_values)
            )
        else:
            items.append(("message", message))
        for position, text in items:
            if action == "replace":
                if teacher_target_rejection_reason(text) or uci_quality_rejection_reason(
                    text,
                    frequencies,
                    single_keep=False,
                ):
                    continue
            else:
                if text_rejection_reason(text) or uci_quality_rejection_reason(
                    text,
                    frequencies,
                    single_keep=word_count(text) == 1,
                ):
                    continue
            result.append(
                Candidate(
                    text=text,
                    target=text,
                    source_dataset="uci_sms_ham",
                    source_record_id=f"{record_id}:{position}",
                    source_license="CC BY 4.0",
                    family_id=f"uci_sms_ham:{record_id}",
                    category="casual_keep" if action == "keep" else "shorthand_target",
                    target_changed=action == "replace",
                )
            )
    return result


def multilexnorm_candidates(
    sentences: Sequence[Sequence[tuple[str, str]]], *, split: str, action: str
) -> list[Candidate]:
    result: list[Candidate] = []
    for sentence_index, sentence in enumerate(sentences):
        for length in range(1, CONTRACT.max_words + 1):
            for start in range(0, len(sentence) - length + 1):
                token_pairs = sentence[start : start + length]
                source = " ".join(item[0] for item in token_pairs).strip()
                target = " ".join(item[1] for item in token_pairs if item[1]).strip()
                changed = normalize_text(source) != normalize_text(target)
                if (action == "replace") != changed:
                    continue
                if action == "replace":
                    if pair_rejection_reason(source, target):
                        continue
                elif text_rejection_reason(source):
                    continue
                result.append(
                    Candidate(
                        text=source,
                        target=target if action == "replace" else source,
                        source_dataset=f"multilexnorm_en_{split}",
                        source_record_id=f"{sentence_index}:{start}:{length}",
                        source_license="CC BY 4.0",
                        family_id=f"multilexnorm_en_{split}:{sentence_index}",
                        category="lexical_normalization"
                        if action == "replace"
                        else "social_keep",
                        target_changed=action == "replace",
                    )
                )
    return result


def jfleg_candidates(parsed: Sequence[tuple[int, int, str, str]]) -> list[Candidate]:
    return [
        Candidate(
            text=source,
            target=target,
            source_dataset="jfleg_test",
            source_record_id=f"{source_index}:{reference_index}",
            source_license="CC BY-NC-SA 4.0",
            family_id=f"jfleg_test:{source_index}",
            category="human_grammar_correction",
            target_changed=True,
        )
        for source_index, reference_index, source, target in parsed
    ]


def choose_candidates(
    candidates: Sequence[Candidate],
    *,
    count: int,
    single_count: int,
    rng: random.Random,
    blocked_texts: Sequence[str] = (),
) -> list[Candidate]:
    ordered = sorted(candidates, key=lambda item: (item.source_record_id, item.text, item.target))
    rng.shuffle(ordered)
    singles = [item for item in ordered if word_count(item.text) == 1]
    multiples = [item for item in ordered if word_count(item.text) > 1]
    blocked_normalized = [normalize_text(value) for value in blocked_texts]
    blocked_exact = set(blocked_normalized)
    selected: list[Candidate] = []
    normalized: set[str] = set()

    def take(pool: Sequence[Candidate], quota: int) -> None:
        for item in pool:
            if len(selected) >= quota:
                return
            norm = normalize_text(item.text)
            if not norm or norm in normalized or norm in blocked_exact:
                continue
            norm_words = norm.split()
            if len(norm_words) >= 3 and any(
                abs(len(other.split()) - len(norm_words)) <= 1
                and SequenceMatcher(None, norm, other, autojunk=False).ratio() >= 0.94
                for other in blocked_normalized
                if len(other.split()) >= 3
            ):
                continue
            normalized.add(norm)
            selected.append(item)

    take(singles, single_count)
    if len(selected) != single_count:
        raise BuildError(f"could not satisfy single-word quota {single_count}")
    take(multiples, count)
    if len(selected) != count:
        raise BuildError(f"could not select {count} unique candidates")
    return selected
