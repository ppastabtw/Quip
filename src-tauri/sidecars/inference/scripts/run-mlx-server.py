"""Quip's single-prefill, five-way KV-cache-fork MLX server."""

import argparse
import hashlib
import ipaddress
import json
import os
import time
from collections import defaultdict
from contextlib import asynccontextmanager

import mlx.core as mx
import uvicorn
from fastapi import FastAPI, HTTPException

from mlx_vlm import apc
from mlx_vlm.generate.ar import (
    GenerationBatch,
    PromptProcessingBatch,
    _clone_or_share_logits_processor,
    _sample_with_positions,
)
from mlx_vlm.prompt_utils import apply_chat_template
from mlx_vlm.server.app import _build_structured_logits_processors
from mlx_vlm.server.generation import GenerationArguments, _PositionedTargetSampler
from mlx_vlm.server.schemas import ChatRequest
from mlx_vlm.tokenizer_utils import make_streaming_detokenizer
from mlx_vlm.utils import load, prepare_inputs


_state = {}
_SUGGESTION_PREFIX = '{"suggestion":"'


# mlx-vlm currently supplies row id 0 for every decode row. Give colliding
# rows independent deterministic keys while preserving its positioned sampler.
_upstream_sample_target = _PositionedTargetSampler.sample_target


def _sample_distinct_batch_rows(self, logprobs, *, row_ids, positions):
    pairs = [
        (int(row_id), int(position))
        for row_id, position in zip(row_ids, positions, strict=True)
    ]
    if len(set(pairs)) == len(pairs):
        return _upstream_sample_target(
            self, logprobs, row_ids=row_ids, positions=positions
        )

    occurrences = defaultdict(int)
    distinct = []
    for pair in pairs:
        ordinal = occurrences[pair]
        occurrences[pair] += 1
        distinct.append((pair[0] << 16) + ordinal)
    return _upstream_sample_target(
        self, logprobs, row_ids=distinct, positions=positions
    )


_PositionedTargetSampler.sample_target = _sample_distinct_batch_rows


def _elapsed_us(started):
    return round((time.perf_counter() - started) * 1_000_000)


def _cache_states(prompt_cache):
    states = []
    for entry in prompt_cache:
        try:
            states.append(entry.state)
        except (AttributeError, TypeError):
            pass
    return states


def _clone_processors(processors, completion_count):
    return [
        [_clone_or_share_logits_processor(processor) for processor in processors]
        for _ in range(completion_count)
    ]


def _apply_processors(logits, processors, token_context):
    if not processors or not any(processors):
        return logits
    processed = []
    for index, row_processors in enumerate(processors):
        row = logits[index : index + 1]
        for processor in row_processors or []:
            row = processor(mx.array(token_context[index]), row)
        processed.append(row)
    return mx.concatenate(processed, axis=0)


def _decode_rows(processor, token_rows):
    texts = []
    for tokens in token_rows:
        detokenizer = make_streaming_detokenizer(processor)
        for token in tokens:
            detokenizer.add_token(token)
        detokenizer.finalize()
        texts.append(detokenizer.text)
    return texts


def _format_prompt(model, processor, messages, gen_args):
    return apply_chat_template(
        processor,
        model.config,
        messages,
        num_images=0,
        num_audios=0,
        video=None,
        tools=None,
        **gen_args.to_template_kwargs(),
    )


def _prepare_prompt(model, processor, request, gen_args):
    messages = [
        {"role": message.role, "content": message.content}
        for message in request.messages
    ]
    formatted_prompt = _format_prompt(model, processor, messages, gen_args)
    add_special_tokens = (
        getattr(processor, "chat_template", None) is None
        if model.config.model_type
        in ["gemma3", "gemma3n", "gemma4", "gemma4_unified"]
        else True
    )
    raw_inputs = prepare_inputs(
        processor,
        prompts=formatted_prompt,
        image_token_index=getattr(model.config, "image_token_index", None),
        add_special_tokens=add_special_tokens,
    )
    input_ids = raw_inputs.get("input_ids")
    pixel_values = raw_inputs.get("pixel_values")
    mask = raw_inputs.get("attention_mask")
    data_kwargs = {
        key: value
        for key, value in raw_inputs.items()
        if key not in ["input_ids", "pixel_values", "attention_mask"]
    }
    embeddings = model.get_input_embeddings(
        input_ids, pixel_values, mask=mask, **data_kwargs
    )
    prompt_kwargs = {
        **data_kwargs,
        **{
            key: value
            for key, value in embeddings.to_dict().items()
            if value is not None
        },
    }
    inputs_embeds = prompt_kwargs.pop("inputs_embeds")
    return (
        input_ids,
        inputs_embeds,
        prompt_kwargs,
        messages,
        add_special_tokens,
        formatted_prompt,
    )


def _tokenize(processor, prompt, add_special_tokens):
    tokenizer = (
        processor.tokenizer if hasattr(processor, "tokenizer") else processor
    )
    return tokenizer.encode(prompt, add_special_tokens=add_special_tokens)


def _parse_bool(value, name):
    normalized = str(value).strip().lower()
    if normalized == "true":
        return True
    if normalized == "false":
        return False
    raise ValueError(f"{name} must be true or false")


def _supports_suggestion_elision(body):
    response_format = body.get("response_format")
    if not isinstance(response_format, dict):
        return False
    json_schema = response_format.get("json_schema")
    if not isinstance(json_schema, dict):
        return False
    schema = json_schema.get("schema")
    if not isinstance(schema, dict):
        return False
    return (
        schema.get("type") == "object"
        and schema.get("required") == ["suggestion"]
        and schema.get("additionalProperties") is False
        and set(schema.get("properties", {})) == {"suggestion"}
        and schema["properties"]["suggestion"].get("type") == "string"
    )


def _compact_suggestion_processors(processors, body):
    if not processors:
        raise RuntimeError("guided JSON generation did not provide a logits processor")
    import llguidance as llg

    schema = body["response_format"]["json_schema"]["schema"]
    grammar = llg.JsonCompiler(
        separators=(",", ":"),
        whitespace_pattern="",
    ).compile(json.dumps(schema, ensure_ascii=False, separators=(",", ":")))
    processor = processors[0]
    return [processor.__class__(grammar, processor.llg_tokenizer)]


def _assistant_continuation_tokens(
    processor,
    messages,
    gen_args,
    formatted_prompt,
    prompt_tokens,
    add_special_tokens,
    continuation,
):
    continued_prompt = processor.apply_chat_template(
        messages + [{"role": "assistant", "content": continuation}],
        tokenize=False,
        add_generation_prompt=False,
        continue_final_message=True,
        **gen_args.to_template_kwargs(),
    )
    continued_tokens = _tokenize(
        processor, continued_prompt, add_special_tokens
    )
    if not continued_prompt.startswith(formatted_prompt):
        raise RuntimeError("assistant continuation changed the prompt text")
    if continued_tokens[: len(prompt_tokens)] != prompt_tokens:
        raise RuntimeError("assistant continuation changed the prompt token boundary")
    fixed_tokens = continued_tokens[len(prompt_tokens) :]
    tokenizer = (
        processor.tokenizer if hasattr(processor, "tokenizer") else processor
    )
    if not fixed_tokens or tokenizer.decode(fixed_tokens) != continuation:
        raise RuntimeError("assistant continuation tokens do not round-trip exactly")
    return fixed_tokens


def _unescaped_quote_index(text):
    escaped = False
    for index, character in enumerate(text):
        if escaped:
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == '"':
            return index
    return None


def _common_prefix_length(*token_rows):
    if not token_rows:
        return 0
    limit = min(len(row) for row in token_rows)
    for index in range(limit):
        value = token_rows[0][index]
        if any(row[index] != value for row in token_rows[1:]):
            return index
    return limit


def _layer_boundaries(model, processor, messages, gen_args, full_tokens, add_special_tokens):
    system_a = [dict(message) for message in messages]
    system_b = [dict(message) for message in messages]
    system_a[-1]["content"] = "__quip_system_probe_alpha__"
    system_b[-1]["content"] = "__quip_system_probe_beta__"
    system_prefix = _common_prefix_length(
        full_tokens,
        _tokenize(
            processor,
            _format_prompt(model, processor, system_a, gen_args),
            add_special_tokens,
        ),
        _tokenize(
            processor,
            _format_prompt(model, processor, system_b, gen_args),
            add_special_tokens,
        ),
    )

    try:
        model_input = json.loads(messages[-1]["content"])
    except (TypeError, json.JSONDecodeError) as error:
        raise ValueError("the Quip user message must be a JSON object") from error
    if not isinstance(model_input, dict) or "text" not in model_input:
        raise ValueError("the Quip user message must contain text")

    context_rows = []
    for probe in ["__quip_draft_probe_alpha__", "__quip_draft_probe_beta__"]:
        probe_input = dict(model_input)
        probe_input["text"] = probe
        probe_messages = [dict(message) for message in messages]
        probe_messages[-1]["content"] = json.dumps(
            probe_input, ensure_ascii=False, separators=(",", ":")
        )
        context_rows.append(
            _tokenize(
                processor,
                _format_prompt(model, processor, probe_messages, gen_args),
                add_special_tokens,
            )
        )
    context_prefix = _common_prefix_length(full_tokens, *context_rows)
    return system_prefix, max(system_prefix, context_prefix)


def _token_hash(tokens):
    digest = hashlib.sha256()
    for token in tokens:
        digest.update(int(token).to_bytes(4, "little", signed=False))
    return digest.hexdigest()


def _slice_prompt_kwargs(prompt_kwargs, start, end, full_length):
    sliced = {}
    for key, value in prompt_kwargs.items():
        if (
            isinstance(value, mx.array)
            and value.ndim >= 2
            and value.shape[1] == full_length
        ):
            sliced[key] = value[:, start:end]
        else:
            sliced[key] = value
    return sliced


def _fresh_cache(model, tokens, embeddings, prompt_kwargs):
    prompt_batch = PromptProcessingBatch(
        model=model.language_model,
        uids=[0],
        input_ids=[tokens],
        max_tokens=[1],
        inputs_embeds=embeddings,
        prompt_kwargs=prompt_kwargs,
        prefill_step_size=None,
    )
    output = prompt_batch.model(
        prompt_batch._input_ids,
        cache=prompt_batch.prompt_cache,
        inputs_embeds=prompt_batch._inputs_embeds,
        **prompt_batch._prompt_kwargs,
    )
    logits = output.logits if hasattr(output, "logits") else output
    mx.eval(logits, _cache_states(prompt_batch.prompt_cache))
    return prompt_batch.prompt_cache, logits[:, -1, :]


def _clone_cache(prompt_cache):
    cloned = apc.snapshot_prompt_cache_row(prompt_cache, 0)
    if cloned is None:
        raise RuntimeError("the model cache cannot be cloned")
    return cloned


def _extend_cache(model, prompt_cache, tokens, embeddings, prompt_kwargs):
    if not tokens:
        raise RuntimeError("a cache layer cannot have an empty suffix")
    output = model.language_model(
        mx.array([tokens]),
        cache=prompt_cache,
        inputs_embeds=embeddings,
        **prompt_kwargs,
    )
    logits = output.logits if hasattr(output, "logits") else output
    mx.eval(logits, _cache_states(prompt_cache))
    return prompt_cache, logits[:, -1, :]


def _teacher_force_cache(model, prompt_cache, tokens):
    if not tokens:
        raise RuntimeError("the fixed assistant prefix cannot be empty")
    output = model.language_model(mx.array([tokens]), cache=prompt_cache)
    logits = output.logits if hasattr(output, "logits") else output
    mx.eval(logits, _cache_states(prompt_cache))
    return prompt_cache, logits[:, -1, :]


def _filter_batch_cache(prompt_cache, keep):
    keep_array = mx.array(keep, mx.int32)
    for entry in prompt_cache:
        entry.filter(keep_array)


def _prime_processors(processor_rows, prompt_tokens, fixed_tokens, logits):
    primed = []
    for row_index, row_processors in enumerate(processor_rows):
        row_logits = logits[row_index : row_index + 1]
        masked = mx.zeros_like(row_logits)
        for processor in row_processors or []:
            masked = processor(mx.array(prompt_tokens), masked)
        for token_index, token in enumerate(fixed_tokens):
            if row_processors and not bool(mx.isfinite(masked[0, token]).item()):
                raise RuntimeError("the fixed assistant prefix violates the JSON grammar")
            masked = (
                row_logits
                if token_index == len(fixed_tokens) - 1
                else mx.zeros_like(row_logits)
            )
            for processor in row_processors or []:
                masked = processor.process_last_token(token, masked)
        primed.append(masked)
    return mx.concatenate(primed, axis=0)


def _sample_rows(sampler, logits, active_uids, positions):
    logprobs = logits - mx.logsumexp(logits, axis=-1, keepdims=True)
    tokens = _sample_with_positions(
        sampler,
        logprobs,
        row_ids=active_uids,
        positions=positions,
    )
    mx.eval(tokens)
    return tokens.tolist()


def _decode_structured_rows(
    model,
    tokenizer,
    prompt_cache,
    logits,
    sampler,
    processor_rows,
    prompt_tokens,
    fixed_tokens,
    max_tokens,
    completion_count,
    elide_schema,
):
    active_uids = list(range(completion_count))
    token_rows = [[] for _ in range(completion_count)]
    token_context = [list(prompt_tokens) for _ in range(completion_count)]
    generated_counts = [0] * completion_count
    closed_values = [None] * completion_count
    dynamic_decode_us = 0
    fixed_prefix_decode_us = 0
    token_budget = max_tokens - len(fixed_tokens) if elide_schema else max_tokens
    if token_budget < 1:
        raise RuntimeError("max_tokens is too small for the fixed JSON prefix")

    if elide_schema:
        token_context = [
            [*prompt_tokens, *fixed_tokens] for _ in range(completion_count)
        ]
        logits = _prime_processors(
            processor_rows, prompt_tokens, fixed_tokens, logits
        )
        positions = [len(fixed_tokens)] * completion_count
    else:
        logits = _apply_processors(logits, processor_rows, token_context)
        positions = [0] * completion_count

    decode_started = time.perf_counter()
    while active_uids:
        iteration_started = time.perf_counter()
        prefixes_complete_before = elide_schema or all(
            token_rows[uid][: len(fixed_tokens)] == fixed_tokens
            for uid in active_uids
        )
        had_open_values = any(closed_values[uid] is None for uid in active_uids)
        if not elide_schema and all(
            generated_counts[uid] < len(fixed_tokens) for uid in active_uids
        ):
            sampled = [
                fixed_tokens[generated_counts[uid]] for uid in active_uids
            ]
            for row_index, token in enumerate(sampled):
                if not bool(mx.isfinite(logits[row_index, token]).item()):
                    raise RuntimeError(
                        "the canonical fixed prefix violates the compact JSON grammar"
                    )
        else:
            sampled = _sample_rows(sampler, logits, active_uids, positions)
        keep_indices = []
        keep_tokens = []
        keep_processors = []
        keep_context = []
        keep_uids = []
        keep_positions = []

        for row_index, (uid, token) in enumerate(zip(active_uids, sampled, strict=True)):
            generated_counts[uid] += 1
            if tokenizer.stopping_criteria(token):
                if closed_values[uid] is None:
                    raise RuntimeError("structured generation stopped before the JSON string closed")
                continue

            token_rows[uid].append(token)
            token_context[row_index].append(token)
            decoded = tokenizer.decode(token_rows[uid])
            value_text = decoded if elide_schema else decoded[len(_SUGGESTION_PREFIX) :]
            if not elide_schema and not decoded.startswith(
                _SUGGESTION_PREFIX[: min(len(decoded), len(_SUGGESTION_PREFIX))]
            ):
                raise RuntimeError("structured generation diverged from the fixed JSON prefix")
            quote_index = _unescaped_quote_index(value_text)
            if quote_index is not None and closed_values[uid] is None:
                closed_values[uid] = value_text[:quote_index]
                if elide_schema:
                    continue

            if generated_counts[uid] >= token_budget:
                if closed_values[uid] is None:
                    raise RuntimeError("structured generation reached max_tokens before closing")
                continue

            keep_indices.append(row_index)
            keep_tokens.append(token)
            keep_processors.append(processor_rows[row_index])
            keep_context.append(token_context[row_index])
            keep_uids.append(uid)
            keep_positions.append(positions[row_index] + 1)

        if keep_indices:
            if len(keep_indices) != len(active_uids):
                _filter_batch_cache(prompt_cache, keep_indices)
            output = model.language_model(
                mx.array(keep_tokens)[:, None], cache=prompt_cache
            )
            next_logits = output.logits if hasattr(output, "logits") else output
            logits = next_logits[:, -1, :]
            logits = _apply_processors(logits, keep_processors, keep_context)
            mx.eval(logits, _cache_states(prompt_cache))
        else:
            prompt_cache.clear()

        iteration_us = _elapsed_us(iteration_started)
        if prefixes_complete_before and had_open_values:
            dynamic_decode_us += iteration_us
        else:
            fixed_prefix_decode_us += iteration_us

        active_uids = keep_uids
        processor_rows = keep_processors
        token_context = keep_context
        positions = keep_positions

    if any(value is None for value in closed_values):
        raise RuntimeError("one or more structured rows did not close the suggestion string")

    if elide_schema:
        texts = [
            f'{_SUGGESTION_PREFIX}{value}"}}' for value in closed_values
        ]
    else:
        texts = [tokenizer.decode(tokens) for tokens in token_rows]

    return {
        "texts": texts,
        "decode_us": _elapsed_us(decode_started),
        "dynamic_decode_us": dynamic_decode_us,
        "fixed_prefix_decode_us": fixed_prefix_decode_us,
        "generated_tokens": sum(generated_counts),
        "avoided_schema_tokens": (
            completion_count * (len(fixed_tokens) + 1) if elide_schema else 0
        ),
    }


def _generation_args(request, processor):
    return GenerationArguments(
        max_tokens=request.max_tokens or 64,
        temperature=request.temperature,
        top_p=request.top_p,
        top_k=request.top_k,
        min_p=request.min_p,
        seed=request.seed,
        enable_thinking=bool(request.enable_thinking),
        logits_processors=_build_structured_logits_processors(request, processor),
    )


def _forked_generate(body):
    total_started = time.perf_counter()
    completion_count = int(body.get("quip_completion_count", 5))
    if not 1 <= completion_count <= 5:
        raise ValueError("quip_completion_count must be between 1 and 5")

    request = ChatRequest.model_validate(body)
    if request.stream:
        raise ValueError("the Quip cache-fork endpoint is non-streaming")
    if any(not isinstance(message.content, str) for message in request.messages):
        raise ValueError("the Quip cache-fork endpoint accepts text messages only")
    if request.model != _state["model_id"]:
        raise ValueError("the requested model is not loaded")

    model = _state["model"]
    processor = _state["processor"]
    tokenizer = _state["tokenizer"]

    prepare_started = time.perf_counter()
    supports_elision = _supports_suggestion_elision(body)
    requested_elision = _parse_bool(
        body.get(
            "quip_schema_token_elision",
            _state["schema_token_elision_default"],
        ),
        "quip_schema_token_elision",
    )
    if requested_elision and not supports_elision:
        raise ValueError(
            "schema-token elision requires Quip's one-key suggestion JSON schema"
        )
    schema_token_elision = requested_elision and supports_elision
    gen_args = _generation_args(request, processor)
    if supports_elision:
        gen_args.logits_processors = _compact_suggestion_processors(
            gen_args.logits_processors, body
        )
    (
        input_ids,
        inputs_embeds,
        prompt_kwargs,
        messages,
        add_special_tokens,
        formatted_prompt,
    ) = _prepare_prompt(model, processor, request, gen_args)
    prompt_tokens = input_ids.squeeze(0).tolist()
    fixed_tokens = (
        _assistant_continuation_tokens(
            processor,
            messages,
            gen_args,
            formatted_prompt,
            prompt_tokens,
            add_special_tokens,
            _SUGGESTION_PREFIX,
        )
        if supports_elision
        else []
    )
    system_prefix, context_prefix = _layer_boundaries(
        model,
        processor,
        messages,
        gen_args,
        prompt_tokens,
        add_special_tokens,
    )
    if not 0 < system_prefix <= context_prefix < len(prompt_tokens):
        raise RuntimeError("safe layered cache boundaries could not be derived")
    base_processors = gen_args.logits_processors or []
    processor_rows = _clone_processors(base_processors, completion_count)
    prepare_us = _elapsed_us(prepare_started)

    prefill_started = time.perf_counter()
    system_key = _token_hash(prompt_tokens[:system_prefix])
    system_entry = _state["system_caches"].get(system_key)
    system_cache_hit = system_entry is not None
    system_prefill_started = time.perf_counter()
    if system_entry is None:
        system_cache, _ = _fresh_cache(
            model,
            prompt_tokens[:system_prefix],
            inputs_embeds[:, :system_prefix],
            _slice_prompt_kwargs(prompt_kwargs, 0, system_prefix, len(prompt_tokens)),
        )
        system_entry = {
            "cache": _clone_cache(system_cache),
            "prefix_len": system_prefix,
        }
        _state["system_caches"][system_key] = system_entry
    system_prefill_us = _elapsed_us(system_prefill_started) if not system_cache_hit else 0

    context_key = _token_hash(prompt_tokens[:context_prefix])
    context_entry = _state.get("context_cache")
    context_cache_hit = (
        context_entry is not None and context_entry["key"] == context_key
    )
    context_prefill_started = time.perf_counter()
    if not context_cache_hit:
        context_cache = _clone_cache(system_entry["cache"])
        context_cache, _ = _extend_cache(
            model,
            context_cache,
            prompt_tokens[system_prefix:context_prefix],
            inputs_embeds[:, system_prefix:context_prefix],
            _slice_prompt_kwargs(
                prompt_kwargs,
                system_prefix,
                context_prefix,
                len(prompt_tokens),
            ),
        )
        context_entry = {
            "key": context_key,
            "cache": _clone_cache(context_cache),
            "prefix_len": context_prefix,
        }
        _state["context_cache"] = context_entry
    context_prefill_us = _elapsed_us(context_prefill_started) if not context_cache_hit else 0

    draft_prefill_started = time.perf_counter()
    completed_cache = _clone_cache(context_entry["cache"])
    completed_cache, logits = _extend_cache(
        model,
        completed_cache,
        prompt_tokens[context_prefix:],
        inputs_embeds[:, context_prefix:],
        _slice_prompt_kwargs(
            prompt_kwargs, context_prefix, len(prompt_tokens), len(prompt_tokens)
        ),
    )
    draft_prefill_us = _elapsed_us(draft_prefill_started)
    prefill_us = _elapsed_us(prefill_started)

    fixed_prefix_prefill_us = 0
    cache_length = len(prompt_tokens)
    if schema_token_elision:
        fixed_prefix_started = time.perf_counter()
        completed_cache, logits = _teacher_force_cache(
            model, completed_cache, fixed_tokens
        )
        fixed_prefix_prefill_us = _elapsed_us(fixed_prefix_started)
        cache_length += len(fixed_tokens)

    batching_started = time.perf_counter()
    forked_cache, _ = apc.make_warm_batch_exact_cache_multi(
        [completed_cache] * completion_count,
        [cache_length] * completion_count,
    )
    if forked_cache is None:
        raise RuntimeError("the model cache cannot be merged into a decode batch")

    batched_logits = mx.broadcast_to(logits, (completion_count, logits.shape[-1]))
    sampler = (
        _PositionedTargetSampler(
            temperature=gen_args.temperature,
            top_p=gen_args.top_p,
            seed=gen_args.seed,
        )
        if gen_args.temperature > 0
        else lambda values: mx.argmax(values, axis=-1)
    )
    batching_us = _elapsed_us(batching_started)

    if supports_elision:
        decoded = _decode_structured_rows(
            model,
            tokenizer,
            forked_cache,
            batched_logits,
            sampler,
            processor_rows,
            prompt_tokens,
            fixed_tokens,
            gen_args.max_tokens,
            completion_count,
            schema_token_elision,
        )
        texts = decoded["texts"]
        decode_us = decoded["decode_us"]
        dynamic_decode_us = decoded["dynamic_decode_us"]
        generated_tokens = decoded["generated_tokens"]
        avoided_schema_tokens = decoded["avoided_schema_tokens"]
    else:
        token_context = [list(prompt_tokens) for _ in range(completion_count)]
        batched_logits = _apply_processors(
            batched_logits, processor_rows, token_context
        )
        first_tokens = _sample_rows(
            sampler,
            batched_logits,
            list(range(completion_count)),
            [0] * completion_count,
        )
        decode_started = time.perf_counter()
        generation_batch = GenerationBatch(
            model=model.language_model,
            uids=list(range(completion_count)),
            inputs=mx.array(first_tokens),
            prompt_cache=forked_cache,
            sampler=sampler,
            stop_criteria=tokenizer.stopping_criteria,
            max_tokens=[gen_args.max_tokens] * completion_count,
            top_logprobs_k=0,
            greedy_sampling=gen_args.temperature == 0,
            token_context=token_context,
            logits_processors=processor_rows,
            thinking_budget_criteria=[None] * completion_count,
        )
        generation_batch.compute_logprobs = False
        token_rows = [[] for _ in range(completion_count)]
        while len(generation_batch) > 0:
            for response in generation_batch.next():
                if response.finish_reason != "stop":
                    token_rows[response.uid].append(response.token)
        decode_us = _elapsed_us(decode_started)
        dynamic_decode_us = decode_us
        generated_tokens = sum(len(tokens) for tokens in token_rows)
        avoided_schema_tokens = 0
        texts = _decode_rows(processor, token_rows)

    postprocess_started = time.perf_counter()
    if supports_elision:
        for text in texts:
            parsed = json.loads(text)
            if set(parsed) != {"suggestion"} or not isinstance(
                parsed["suggestion"], str
            ):
                raise RuntimeError("structured output violated the suggestion contract")
    postprocess_us = _elapsed_us(postprocess_started)
    total_us = _elapsed_us(total_started)
    accounted = (
        prepare_us
        + prefill_us
        + fixed_prefix_prefill_us
        + batching_us
        + decode_us
        + postprocess_us
    )

    return {
        "id": "quip-cache-fork",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": request.model,
        "choices": [
            {
                "index": index,
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": text},
            }
            for index, text in enumerate(texts)
        ],
        "usage": {
            "prompt_tokens": len(prompt_tokens),
            "completion_tokens": generated_tokens,
            "total_tokens": len(prompt_tokens) + generated_tokens,
        },
        "quip_timings": {
            "system_cache_hit": system_cache_hit,
            "context_cache_hit": context_cache_hit,
            "schema_token_elision": schema_token_elision,
            "prepare_us": prepare_us,
            "prefill_us": prefill_us,
            "system_prefill_us": system_prefill_us,
            "context_prefill_us": context_prefill_us,
            "draft_prefill_us": draft_prefill_us,
            "fixed_prefix_prefill_us": fixed_prefix_prefill_us,
            "batching_us": batching_us,
            "decode_us": decode_us,
            "dynamic_decode_us": dynamic_decode_us,
            "generated_tokens": generated_tokens,
            "avoided_schema_tokens": avoided_schema_tokens,
            "postprocess_us": postprocess_us,
            "overhead_us": max(0, total_us - accounted),
            "total_us": total_us,
        },
    }


def build_app(model_id, adapter_path):
    schema_token_elision_default = _parse_bool(
        os.environ.get("QUIP_SCHEMA_TOKEN_ELISION", "false"),
        "QUIP_SCHEMA_TOKEN_ELISION",
    )

    @asynccontextmanager
    async def lifespan(_app):
        model, processor = load(model_id, adapter_path=adapter_path)
        tokenizer = (
            processor.tokenizer if hasattr(processor, "tokenizer") else processor
        )
        eos_tokens = getattr(model.config, "eos_token_id", None)
        if eos_tokens is not None:
            tokenizer.stopping_criteria.add_eos_token_ids(
                eos_tokens if isinstance(eos_tokens, list) else {eos_tokens}
            )
        _state.update(
            {
                "model": model,
                "processor": processor,
                "tokenizer": tokenizer,
                "model_id": model_id,
                "adapter_path": adapter_path,
                "schema_token_elision_default": schema_token_elision_default,
                "system_caches": {},
                "context_cache": None,
            }
        )
        yield
        _state.clear()

    server = FastAPI(lifespan=lifespan)

    @server.get("/health")
    async def health():
        return {
            "status": "healthy",
            "loaded_model": _state.get("model_id"),
            "loaded_adapter": _state.get("adapter_path"),
            "quip_cache_fork": True,
            "quip_schema_token_elision": _state.get(
                "schema_token_elision_default"
            ),
        }

    @server.post("/v1/quip/completions")
    async def quip_completions(body: dict):
        try:
            return _forked_generate(body)
        except (ValueError, RuntimeError) as error:
            raise HTTPException(status_code=400, detail=str(error)) from error

    return server


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--adapter-path")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=1234)
    return parser.parse_args()


if __name__ == "__main__":
    args = parse_args()
    try:
        bind_address = ipaddress.ip_address(args.host)
    except ValueError as error:
        raise SystemExit("--host must be a loopback IP address") from error
    if not bind_address.is_loopback:
        raise SystemExit("--host must be a loopback IP address")
    uvicorn.run(
        build_app(args.model, args.adapter_path),
        host=args.host,
        port=args.port,
        workers=1,
        server_header=False,
    )
