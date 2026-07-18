"""Shared fixtures for dataset compiler tests."""

from __future__ import annotations

import json

from dataset_compiler import backboard
from dataset_compiler.contract import Candidate


def candidate(index: int = 0, text: str = "please call me") -> Candidate:
    return Candidate(
        text=text,
        target=text,
        source_dataset="fixture",
        source_record_id=f"fixture:{index}",
        source_license="CC BY 4.0",
        family_id=f"fixture:{index}",
        category="fixture",
        target_changed=True,
    )


def response_payload(content, request_id="response-1", model_name=None):
    return {
        "message_id": request_id,
        "thread_id": f"thread-{request_id}",
        "assistant_id": "assistant-1",
        "status": "COMPLETED",
        "content": json.dumps(content),
        "model_provider": backboard.BACKBOARD_LLM_PROVIDER,
        "model_name": model_name or backboard.GENERATION_MODEL,
        "input_tokens": 10,
        "output_tokens": 5,
        "total_tokens": 15,
    }
