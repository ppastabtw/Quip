import json
import tempfile
from pathlib import Path
from unittest import mock

import httpx
import pytest

from dataset_compiler import backboard, compiler
from dataset_compiler.contract import BuildError
from tests.dataset_compiler.helpers import candidate, response_payload


def test_rejected_batch_regenerates_once_then_fails():
    calls = []

    def handler(request):
        calls.append(request)
        body = json.loads(request.content)
        user = json.loads(body["content"])
        if "targets" in user:
            content = {
                "drafts": [
                    {"target_id": item["target_id"], "draft": "plz call me"}
                    for item in user["targets"]
                ]
            }
        else:
            content = {
                "verdicts": [
                    {
                        "target_id": item["target_id"],
                        "target_intentional": False,
                        "meaning_preserved": False,
                        "facts_preserved": True,
                        "tone_preserved": True,
                        "natural": True,
                        "minimal": True,
                        "accepted": False,
                        "reason": "meaning",
                    }
                    for item in user["pairs"]
                ]
            }
        return httpx.Response(
            200,
            request=request,
            json=response_payload(
                content,
                str(len(calls)),
                model_name=body["model_name"],
            ),
        )

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
            with pytest.raises(BuildError):
                compiler.augment_targets([candidate()], client)
        finally:
            client.close()
    assert len(calls) == 3
