"""Shared fixtures for dataset compiler tests."""

from __future__ import annotations

from dataset_compiler.contract import Candidate


def candidate(index: int = 0, text: str = "please call me") -> Candidate:
    return Candidate(
        text=text,
        target=text,
        source_dataset="fixture",
        source_record_id=f"fixture:{index}",
        source_partition="train",
        source_license="CC BY 4.0",
        family_id=f"fixture:{index}",
        category="natural_keep",
        target_changed=False,
    )
