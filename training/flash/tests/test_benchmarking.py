import json
from pathlib import Path

import httpx

from benchmarking import (
    BackboardTransport,
    ModelSpec,
    backboard_request_payload,
    freesolo_request_payload,
    load_config,
    markdown_report,
    model_runtime_summary,
    validate_dataset,
)
from benchmark_dashboard import render_dashboard


ROOT = Path(__file__).resolve().parents[1]


def test_committed_matrix_covers_qwen_sizes():
    config = load_config(ROOT / "benchmarks" / "models.toml")
    models = {spec.model for spec in config.models}
    assert {
        "Qwen/Qwen3.5-0.8B",
        "Qwen/Qwen3.5-2B",
        "Qwen/Qwen3.5-4B",
        "Qwen/Qwen3.5-9B",
        "Qwen/Qwen3.6-35B-A3B",
    } <= models


def test_payload_uses_prompt_json_schema_and_non_thinking_mode():
    row = {"input": '{"text":"cnt cm tmrw"}'}
    freesolo = freesolo_request_payload(row, "Qwen/Qwen3.5-2B", 128)
    assert freesolo["messages"][0]["role"] == "system"
    assert freesolo["messages"][1] == {"role": "user", "content": row["input"]}
    assert freesolo["temperature"] == 0.0
    assert freesolo["chat_template_kwargs"] == {"enable_thinking": False}
    assert freesolo["response_format"]["type"] == "json_schema"


def test_backboard_payload_is_stateless_json_and_non_thinking():
    row = {"input": '{"text":"cnt cm tmrw"}'}
    spec = ModelSpec("Fixture", "backboard", "fixture-model", "fixture-provider")
    payload = backboard_request_payload(row, spec)
    assert payload["system_prompt"] == freesolo_request_payload(
        row, "Qwen/Qwen3.5-2B", 128
    )["messages"][0]["content"]
    assert payload["content"] == row["input"]
    assert payload["json_output"] is True
    assert payload["memory"] == "off"
    assert payload["web_search"] == "off"
    assert "thinking" not in payload


def test_backboard_catalog_validation_and_completion_capture_cost():
    spec = ModelSpec("Fixture", "backboard", "fixture-model", "fixture-provider")
    message_requests = []

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/api/models":
            return httpx.Response(
                200,
                request=request,
                json={
                    "models": [
                        {
                            "provider": "fixture-provider",
                            "name": "fixture-model",
                            "input_cost_per_1m_tokens": 5,
                            "output_cost_per_1m_tokens": 30,
                        }
                    ]
                },
            )
        message_requests.append(json.loads(request.content))
        return httpx.Response(
            200,
            request=request,
            json={
                "status": "COMPLETED",
                "content": '{"suggestion":"Come tomorrow"}',
                "model_provider": "fixture-provider",
                "model_name": "fixture-model",
                "input_tokens": 100,
                "output_tokens": 10,
                "total_tokens": 110,
            },
        )

    client = BackboardTransport(
        timeout_seconds=1,
        api_key="secret",
        transport=httpx.MockTransport(handler),
        sleep=lambda _: None,
    )
    try:
        client.validate_models([spec])
        result = client.complete({"input": '{"text":"cm tmrw"}'}, spec, 128)
    finally:
        client.close()
    assert result["estimated_cost_usd"] == 0.0008
    assert result["returned_model"] == "fixture-model"
    assert message_requests == [
        {
            **backboard_request_payload({"input": '{"text":"cm tmrw"}'}, spec),
            "memory": "off",
        }
    ]


def test_backboard_transport_forces_memory_off_for_every_message_request():
    requests = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(json.loads(request.content))
        return httpx.Response(200, request=request, json={})

    client = BackboardTransport(
        timeout_seconds=1,
        api_key="secret",
        transport=httpx.MockTransport(handler),
        sleep=lambda _: None,
    )
    try:
        client._request(
            "POST", "/threads/messages", json_payload={"memory": "Auto"}
        )
        client._request("POST", "/threads/messages", json_payload={"content": "hi"})
    finally:
        client.close()

    assert requests == [
        {"memory": "off"},
        {"content": "hi", "memory": "off"},
    ]


def test_dataset_gold_outputs_pass_benchmark_contract():
    rows = [json.loads(line) for line in (ROOT / "dataset" / "eval.jsonl").read_text(encoding="utf-8").splitlines()]
    validate_dataset(rows)


def test_runtime_and_markdown_reports_include_latency_errors_and_cost():
    runtime = model_runtime_summary(
        [
            {
                "latency_ms": 100,
                "input_tokens": 10,
                "output_tokens": 2,
                "estimated_cost_usd": 0.001,
                "error": None,
            },
            {
                "latency_ms": 300,
                "input_tokens": 20,
                "output_tokens": 4,
                "estimated_cost_usd": 0.002,
                "error": None,
            },
        ]
    )
    assert runtime["median_latency_ms"] == 200
    assert runtime["p95_latency_ms"] == 300
    assert runtime["estimated_cost_usd"] == 0.003

    report = markdown_report(
        {
            "models": [
                {
                    "label": "fixture",
                    "transport": "freesolo",
                    "metrics": {
                        "overall_success": 1.0,
                        "decode_success": 1.0,
                        "unnecessary_edit_rate": 0.0,
                        "schema_validity": 1.0,
                        "mean_latency_ms": 200,
                    },
                    "runtime": runtime,
                }
            ]
        }
    )
    assert "| fixture | freesolo | 100.0%" in report

    dashboard = render_dashboard(
        {
            "created_at": "2026-07-18T12:00:00+00:00",
            "dataset": "dataset/eval.jsonl",
            "examples": 2,
            "models": [
                {
                    "label": "fixture",
                    "transport": "freesolo",
                    "provider": None,
                    "model": "fixture-model",
                    "metrics": {
                        "overall_success": 1.0,
                        "decode_success": 1.0,
                        "unnecessary_edit_rate": 0.0,
                        "schema_validity": 1.0,
                        "mean_latency_ms": 200,
                        "categories": {"typo": {"success_rate": 1.0}},
                    },
                    "runtime": runtime,
                }
            ],
        }
    )
    assert "Quality versus latency" in dashboard
    assert "<h1>Quip benchmark</h1>" in dashboard
    assert "Quality, restraint, and response time" not in dashboard
    assert "Select a header to sort" not in dashboard
    assert "fixture-model" in dashboard
    assert "freesolo" in dashboard
