import json
import tempfile
from pathlib import Path
from unittest import mock

import httpx
import pytest

from dataset_compiler import backboard, compiler
from dataset_compiler.contract import BuildError
from tests.dataset_compiler.helpers import candidate, response_payload


def test_generation_and_verification_schema_validation():
    drafts = backboard.validate_generation_payload(
        {"drafts": [{"target_id": "a", "draft": "plz call"}]}, ["a"]
    )
    assert drafts == {"a": "plz call"}
    verdict = {
        "target_id": "a",
        "target_intentional": True,
        "meaning_preserved": True,
        "facts_preserved": True,
        "tone_preserved": True,
        "natural": True,
        "minimal": True,
        "accepted": True,
        "reason": "ok",
    }
    assert backboard.validate_verification_payload({"verdicts": [verdict]}, ["a"])["a"][
        "accepted"
    ]
    with pytest.raises(BuildError):
        backboard.validate_generation_payload({"drafts": []}, ["a"])


def test_request_disables_memory_and_pins_phase_model():
    generation = backboard.build_request_payload(
        system_prompt="fixture", user_payload={}, phase="generation"
    )
    verification = backboard.build_request_payload(
        system_prompt="fixture", user_payload={}, phase="verification"
    )
    assert generation["memory"] == "off"
    assert verification["memory"] == "off"
    assert generation["model_name"] == backboard.GENERATION_MODEL
    assert verification["model_name"] == backboard.VERIFICATION_MODEL
    forbidden = {
        "memory_pro",
        "memory_response_citation",
        "memory_citation",
        "assistant_id",
        "thread_id",
    }
    assert forbidden.isdisjoint(generation)
    assert forbidden.isdisjoint(verification)


def test_retries_5xx_and_does_not_retry_4xx():
    calls = []

    def retry_handler(request):
        calls.append(request)
        if len(calls) < 3:
            return httpx.Response(500, request=request)
        body = json.loads(request.content)
        return httpx.Response(
            200,
            request=request,
            json=response_payload({"drafts": []}, model_name=body["model_name"]),
        )

    with tempfile.TemporaryDirectory() as directory, mock.patch.object(
        backboard, "TEACHER_CACHE_DIR", Path(directory)
    ):
        client = backboard.BackboardClient(
            offline=False,
            api_key="secret",
            transport=httpx.MockTransport(retry_handler),
            sleep=lambda _: None,
        )
        try:
            assert client.complete(
                system_prompt="fixture", user_payload={}, phase="generation"
            ) == {"drafts": []}
        finally:
            client.close()
    assert len(calls) == 3
    assert client.retry_count == 2

    def auth_handler(request):
        return httpx.Response(401, request=request)

    with tempfile.TemporaryDirectory() as directory, mock.patch.object(
        backboard, "TEACHER_CACHE_DIR", Path(directory)
    ):
        client = backboard.BackboardClient(
            offline=False,
            api_key="secret",
            transport=httpx.MockTransport(auth_handler),
            sleep=lambda _: None,
        )
        try:
            with pytest.raises(httpx.HTTPStatusError):
                client.complete(
                    system_prompt="auth", user_payload={}, phase="verification"
                )
        finally:
            client.close()
    assert client.network_request_count == 1


def test_http_200_failed_status_is_retried():
    calls = []

    def handler(request):
        calls.append(request)
        body = json.loads(request.content)
        payload = response_payload({"drafts": []}, model_name=body["model_name"])
        if len(calls) < 3:
            payload["status"] = "FAILED"
        return httpx.Response(200, request=request, json=payload)

    with tempfile.TemporaryDirectory() as directory, mock.patch.object(
        backboard, "TEACHER_CACHE_DIR", Path(directory)
    ):
        client = backboard.BackboardClient(
            offline=False,
            api_key="secret",
            transport=httpx.MockTransport(handler),
            sleep=lambda _: None,
        )
        try:
            assert client.complete(
                system_prompt="fixture", user_payload={}, phase="generation"
            ) == {"drafts": []}
        finally:
            client.close()
    assert len(calls) == 3
    assert client.retry_count == 2


def test_cache_reuse_makes_augmentation_deterministic():
    network_calls = []

    def handler(request):
        network_calls.append(request)
        body = json.loads(request.content)
        user = json.loads(body["content"])
        if "targets" in user:
            content = {
                "drafts": [
                    {
                        "target_id": item["target_id"],
                        "draft": item["target"].replace("please", "plz", 1),
                    }
                    for item in user["targets"]
                ]
            }
        else:
            content = {
                "verdicts": [
                    {
                        "target_id": item["target_id"],
                        "target_intentional": True,
                        "meaning_preserved": True,
                        "facts_preserved": True,
                        "tone_preserved": True,
                        "natural": True,
                        "minimal": True,
                        "accepted": True,
                        "reason": "ok",
                    }
                    for item in user["pairs"]
                ]
            }
        return httpx.Response(
            200,
            request=request,
            json=response_payload(content, model_name=body["model_name"]),
        )

    targets = [candidate(index, f"please call me word{chr(97 + index)}") for index in range(20)]
    with tempfile.TemporaryDirectory() as directory, mock.patch.object(
        backboard, "TEACHER_CACHE_DIR", Path(directory)
    ):
        first_client = backboard.BackboardClient(
            offline=False,
            api_key="secret",
            transport=httpx.MockTransport(handler),
            sleep=lambda _: None,
        )
        try:
            first = compiler.augment_targets(targets, first_client)
        finally:
            first_client.close()
        second_client = backboard.BackboardClient(offline=True)
        try:
            second = compiler.augment_targets(targets, second_client)
        finally:
            second_client.close()
    assert first == second
    assert len(network_calls) == 2
    assert second_client.cache_hits == 2


def test_cache_identity_includes_pinned_phase_model():
    calls = []

    def handler(request):
        calls.append(request)
        body = json.loads(request.content)
        content = (
            {"drafts": []}
            if body["model_name"] == backboard.GENERATION_MODEL
            else {"verdicts": []}
        )
        return httpx.Response(
            200,
            request=request,
            json=response_payload(content, model_name=body["model_name"]),
        )

    with tempfile.TemporaryDirectory() as directory, mock.patch.object(
        backboard, "TEACHER_CACHE_DIR", Path(directory)
    ):
        client = backboard.BackboardClient(
            offline=False,
            api_key="secret",
            transport=httpx.MockTransport(handler),
        )
        try:
            client.complete(system_prompt="fixture", user_payload={}, phase="generation")
            client.complete(system_prompt="fixture", user_payload={}, phase="verification")
        finally:
            client.close()
        envelopes = [json.loads(path.read_text()) for path in Path(directory).glob("*.json")]
    assert len(calls) == 2
    assert len(envelopes) == 2
    assert {envelope["model_name"] for envelope in envelopes} == {
        backboard.GENERATION_MODEL,
        backboard.VERIFICATION_MODEL,
    }
    for envelope in envelopes:
        assert "headers" not in envelope
        assert "api_key" not in envelope
        assert "authorization" not in json.dumps(envelope).casefold()
        assert "secret" not in json.dumps(envelope)
