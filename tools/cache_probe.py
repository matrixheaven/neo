#!/usr/bin/env python3
"""DeepSeek Anthropic-compatible cache probe.

A local, opt-in test utility that observes the real Anthropic Messages
requests Neo sends, relays the real streamed response back without buffering
it to completion, and produces both a live local web view and a
machine-readable report.

Maintained files:
    tools/cache_probe.py
    tools/cache_probe.html

Generated evidence (never committed):
    target/cache-probe/<run-id>/
        events.jsonl
        report.json
        requests/0001.json ...

Python 3.11+ standard library only. The script owns analysis, forwarding,
persistence, and reporting; the HTML page is presentation only.

WARNING: full request bodies are stored locally because exact historical
comparison is the purpose of this tool. They may contain private source
code, prompts, and tool output.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import threading
import time
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Optional
from urllib.parse import urlsplit

MAX_REQUEST_BYTES = 64 * 1024 * 1024
MAX_DIFF_PATHS = 100
MAX_STREAM_CHUNK = 16_384

HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}

USAGE_FIELDS = (
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
)

DERIVED_FIELDS = (
    "cache_hit_tokens",
    "new_cache_tokens",
    "non_hit_tokens",
    "observed_input_tokens",
)

REPORT_TOP_LEVEL = ("run", "summary", "sequences", "requests", "tool_summary", "warnings")


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def canonical_bytes(value: object) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def json_hash(value: object) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def identity_key(body: dict[str, object], route: str) -> tuple[object, ...]:
    metadata = body.get("metadata")
    user_id = metadata.get("user_id") if isinstance(metadata, dict) else None
    return route, body.get("model"), user_id


def first_message_anchor(body: dict[str, object]) -> Optional[bytes]:
    messages = body.get("messages")
    if not isinstance(messages, list) or not messages:
        return None
    return canonical_bytes(messages[0])


def common_message_prefix_length(left: list[object], right: list[object]) -> int:
    length = 0
    for a, b in zip(left, right):
        if canonical_bytes(a) != canonical_bytes(b):
            break
        length += 1
    return length


def truncate_to_predecessor(body: dict[str, object], n: int) -> dict[str, object]:
    truncated = dict(body)
    messages = body.get("messages")
    if isinstance(messages, list):
        truncated["messages"] = messages[:n]
    return truncated


def select_predecessor(
    candidates: list[dict[str, object]],
    body: dict[str, object],
    route: str,
) -> tuple[Optional[dict[str, object]], Optional[str]]:
    """Find the conservative predecessor for `body`.

    Order:
    1. Filter candidates by route, model, and metadata user identifier.
    2. Prefer candidates whose entire historical messages array is an exact
       prefix of the current messages array; choose the longest such prefix,
       then the newest request.
    3. When no exact prefix exists, allow a mutation candidate only when
       exactly one established sequence under the same identity has the same
       non-null first-message anchor.
    4. Otherwise return (None, None): a new sequence with unknown prefix.

    Returns (predecessor_record, kind) where kind is "exact", "mutation", or
    None.
    """
    key = identity_key(body, route)
    same_identity = [c for c in candidates if identity_key(c["body"], route) == key]

    messages = body.get("messages")
    if not isinstance(messages, list):
        messages = []

    # 2. Exact historical-message prefix.
    exact: Optional[dict[str, object]] = None
    for cand in same_identity:
        cand_messages = cand["body"].get("messages")
        if not isinstance(cand_messages, list) or not cand_messages:
            continue
        if len(cand_messages) > len(messages):
            continue
        if common_message_prefix_length(cand_messages, messages) != len(cand_messages):
            continue
        if exact is None:
            exact = cand
            continue
        exact_len = len(exact["body"]["messages"])
        cand_len = len(cand_messages)
        if cand_len > exact_len or (
            cand_len == exact_len
            and int(cand["request_id"]) > int(exact["request_id"])
        ):
            exact = cand
    if exact is not None:
        return exact, "exact"

    # 3. Conservative first-message anchor mutation candidate.
    anchor = first_message_anchor(body)
    if anchor is not None:
        # One representative (latest) request per established sequence.
        sequence_latest: dict[str, dict[str, object]] = {}
        for cand in same_identity:
            seq = str(cand["sequence_id"])
            if seq not in sequence_latest or int(cand["request_id"]) > int(
                sequence_latest[seq]["request_id"]
            ):
                sequence_latest[seq] = cand
        matches = [
            c for c in sequence_latest.values() if first_message_anchor(c["body"]) == anchor
        ]
        if len(matches) == 1:
            return matches[0], "mutation"

    # 4. Ambiguous or no safe predecessor.
    return None, None


def diff_json_paths(
    before: object, after: object, path: str = "$"
) -> tuple[bool, Optional[str], list[str]]:
    """Compare two JSON values canonically.

    Returns (equal, first_changed_path, all_changed_paths). Object keys are
    compared in sorted order; arrays preserve their order. Changed paths are
    collected up to MAX_DIFF_PATHS.
    """
    if type(before) is not type(after):
        return False, path, [path]

    if isinstance(before, dict):
        keys = sorted(set(before) | set(after))
        equal = True
        first: Optional[str] = None
        paths: list[str] = []
        for key in keys:
            child = f"{path}.{key}"
            if key not in before or key not in after:
                changed = child
            else:
                sub_equal, sub_first, sub_paths = diff_json_paths(before[key], after[key], child)
                if sub_equal:
                    continue
                changed = sub_first if sub_first is not None else child
                paths.extend(sub_paths)
                equal = False
                if first is None:
                    first = changed
                continue
            equal = False
            paths.append(changed)
            if first is None:
                first = changed
        return equal, first, paths[:MAX_DIFF_PATHS]

    if isinstance(before, list):
        equal = True
        first: Optional[str] = None
        paths: list[str] = []
        width = max(len(before), len(after))
        for index in range(width):
            child = f"{path}[{index}]"
            if index >= len(before) or index >= len(after):
                changed = child
            else:
                sub_equal, sub_first, sub_paths = diff_json_paths(
                    before[index], after[index], child
                )
                if sub_equal:
                    continue
                changed = sub_first if sub_first is not None else child
                paths.extend(sub_paths)
                equal = False
                if first is None:
                    first = changed
                continue
            equal = False
            paths.append(changed)
            if first is None:
                first = changed
        return equal, first, paths[:MAX_DIFF_PATHS]

    if before != after:
        return False, path, [path]
    return True, None, []


def summarize_increment(
    predecessor_body: dict[str, object], current_body: dict[str, object]
) -> dict[str, object]:
    """Summarize the current messages tail after the predecessor length."""
    pred_messages = predecessor_body.get("messages")
    cur_messages = current_body.get("messages")
    if not isinstance(pred_messages, list):
        pred_messages = []
    if not isinstance(cur_messages, list):
        cur_messages = []
    tail = cur_messages[len(pred_messages):]

    blocks: dict[str, int] = {}
    tool_uses: list[dict[str, object]] = []
    tool_results: list[dict[str, object]] = []
    user_text_bytes = 0
    assistant_text_bytes = 0
    thinking_bytes = 0

    for message in tail:
        if not isinstance(message, dict):
            continue
        role = message.get("role")
        content = message.get("content")
        if isinstance(content, str):
            blocks["text"] = blocks.get("text", 0) + 1
            if role == "user":
                user_text_bytes += len(content.encode("utf-8"))
            else:
                assistant_text_bytes += len(content.encode("utf-8"))
        elif isinstance(content, list):
            for block in content:
                if not isinstance(block, dict):
                    continue
                block_type = block.get("type")
                blocks[str(block_type)] = blocks.get(str(block_type), 0) + 1
                if block_type == "text":
                    text = block.get("text")
                    if isinstance(text, str):
                        if role == "user":
                            user_text_bytes += len(text.encode("utf-8"))
                        else:
                            assistant_text_bytes += len(text.encode("utf-8"))
                elif block_type == "thinking":
                    thinking = block.get("thinking")
                    if isinstance(thinking, str):
                        thinking_bytes += len(thinking.encode("utf-8"))
                elif block_type == "tool_use":
                    tool_uses.append(
                        {"id": block.get("id"), "name": block.get("name")}
                    )
                elif block_type == "tool_result":
                    result_content = block.get("content")
                    size = 0
                    if result_content is not None:
                        size = len(canonical_bytes(result_content))
                    tool_results.append(
                        {"id": block.get("tool_use_id"), "bytes": size}
                    )

    return {
        "appended_messages": len(tail),
        "canonical_bytes": len(canonical_bytes(tail)),
        "blocks": blocks,
        "user_text_bytes": user_text_bytes,
        "assistant_text_bytes": assistant_text_bytes,
        "thinking_bytes": thinking_bytes,
        "tool_uses": tool_uses,
        "tool_results": tool_results,
    }


def resolve_tool_names(
    messages: list[object], tool_result_ids: list[object]
) -> dict[object, Optional[str]]:
    """Resolve tool-result identifiers to tool names from tool-use blocks
    already present in the current message history. Unresolved identifiers
    stay unresolved and are never assigned a guessed tool."""
    id_to_name: dict[object, str] = {}
    for message in messages:
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                block_id = block.get("id")
                name = block.get("name")
                if block_id is not None and isinstance(name, str):
                    id_to_name.setdefault(block_id, name)
    return {rid: id_to_name.get(rid) for rid in tool_result_ids}


def tool_attribution(
    increment: Optional[dict[str, object]], messages: list[object]
) -> list[str]:
    """Names of tools involved in the appended increment, de-duplicated."""
    if increment is None:
        return []
    result_ids = [
        tr["id"] for tr in increment.get("tool_results", []) if tr.get("id") is not None
    ]
    resolved = resolve_tool_names(messages, result_ids)
    names: list[str] = []
    for tu in increment.get("tool_uses", []):
        name = tu.get("name")
        if isinstance(name, str):
            names.append(name)
    for name in resolved.values():
        if isinstance(name, str):
            names.append(name)
    seen: set[str] = set()
    ordered: list[str] = []
    for name in names:
        if name not in seen:
            seen.add(name)
            ordered.append(name)
    return ordered


def merge_usage_event(
    current: Optional[dict[str, object]], event: dict[str, object]
) -> dict[str, object]:
    """Merge one streamed usage object into the running per-request usage.

    Missing fields remain missing (None). Fields already merged keep their
    value unless the new event provides a non-null one.
    """
    result = dict(current or {})
    for key, value in event.items():
        if value is not None:
            result[key] = value
    return result


def derive_usage(usage: Optional[dict[str, object]]) -> dict[str, Optional[object]]:
    """Derive the four approved values from raw usage fields.

    Missing raw fields leave the derived value null; zero values are never
    invented.
    """
    if not usage:
        return {field: None for field in DERIVED_FIELDS}

    def add(a: object, b: object) -> Optional[object]:
        if a is None or b is None:
            return None
        return a + b

    return {
        "cache_hit_tokens": usage.get("cache_read_input_tokens"),
        "new_cache_tokens": usage.get("cache_creation_input_tokens"),
        "non_hit_tokens": add(
            usage.get("input_tokens"), usage.get("cache_creation_input_tokens")
        ),
        "observed_input_tokens": add(
            add(usage.get("input_tokens"), usage.get("cache_read_input_tokens")),
            usage.get("cache_creation_input_tokens"),
        ),
    }


def population_stats(values: list[float]) -> dict[str, Optional[float]]:
    n = len(values)
    if n == 0:
        return {"n": 0, "mean": None, "variance": None, "stdev": None}
    mean = sum(values) / n
    variance = sum((v - mean) ** 2 for v in values) / n
    return {
        "n": n,
        "mean": mean,
        "variance": variance,
        "stdev": variance ** 0.5,
    }


def detect_spike(current_non_hit: float, previous_samples: list[float]) -> bool:
    """A numeric spike requires five earlier usable samples and:
    current non_hit > previous mean + 3 * previous standard deviation.
    When the previous standard deviation is zero, any value greater than the
    previous mean is a spike.
    """
    if len(previous_samples) < 5:
        return False
    previous = population_stats(previous_samples)
    assert previous["mean"] is not None and previous["stdev"] is not None
    if previous["stdev"] == 0:
        return current_non_hit > previous["mean"]
    return current_non_hit > previous["mean"] + 3 * previous["stdev"]


def analyze_request(
    body: dict[str, object],
    route: str,
    predecessor: Optional[dict[str, object]],
    kind: Optional[str],
    sequence_id: str,
    sequence_position: int,
) -> dict[str, object]:
    """Analyze one request against its selected predecessor.

    `predecessor` records carry at least request_id, sequence_id, body, and
    hash keys.
    """
    messages = body.get("messages")
    if not isinstance(messages, list):
        messages = []

    metadata = body.get("metadata")
    user_id = metadata.get("user_id") if isinstance(metadata, dict) else None

    result: dict[str, object] = {
        "sequence_id": sequence_id,
        "sequence_position": sequence_position,
        "predecessor_id": None,
        "predecessor_kind": None,
        "prefix_status": "unknown",
        "first_changed_path": None,
        "changed_paths": [],
        "common_prefix_length": None,
        "hash": json_hash(body),
        "predecessor_hash": None,
        "increment": None,
        "tools": [],
        "model": body.get("model"),
        "user_id": user_id,
    }

    if predecessor is None:
        return result

    pred_messages = predecessor["body"].get("messages")
    if not isinstance(pred_messages, list):
        pred_messages = []

    result["predecessor_id"] = predecessor["request_id"]
    result["predecessor_kind"] = kind
    result["predecessor_hash"] = predecessor.get("hash")

    if kind == "exact":
        truncated = truncate_to_predecessor(body, len(pred_messages))
        equal, first, paths = diff_json_paths(predecessor["body"], truncated)
        result["prefix_status"] = "stable" if equal else "changed"
        result["first_changed_path"] = first
        result["changed_paths"] = paths
        result["common_prefix_length"] = len(pred_messages)
        increment = summarize_increment(predecessor["body"], body)
        result["increment"] = increment
        result["tools"] = tool_attribution(increment, messages)
    else:
        # Mutation candidate: the historical messages were rewritten, so no
        # safe tail increment exists. Compare the complete current body.
        equal, first, paths = diff_json_paths(predecessor["body"], body)
        result["prefix_status"] = "changed"
        result["first_changed_path"] = first
        result["changed_paths"] = paths
        result["common_prefix_length"] = common_message_prefix_length(
            pred_messages, messages
        )
    return result


class RunStore:
    """Thread-safe run state: request persistence, lineage, analysis,
    usage, statistics, events, and atomic report writes."""

    def __init__(
        self,
        output_root: Path,
        upstream_base: Optional[str] = None,
        run_id: Optional[str] = None,
    ) -> None:
        self._lock = threading.RLock()
        self.output_root = Path(output_root)
        self.run_id = run_id or (
            time.strftime("%Y%m%d-%H%M%S", time.gmtime()) + "-" + uuid.uuid4().hex[:6]
        )
        self.run_dir = self.output_root / self.run_id
        self.requests_dir = self.run_dir / "requests"
        self.events_path = self.run_dir / "events.jsonl"
        self.report_path = self.run_dir / "report.json"
        self.upstream_base = upstream_base
        self.started_at = now_iso()
        self.requests: dict[int, dict[str, object]] = {}
        self.sequences: dict[str, dict[str, object]] = {}
        self.warnings: list[dict[str, object]] = []
        self.requests_dir.mkdir(parents=True, exist_ok=True)
        self.append_event(
            {"event": "run_start", "run_id": self.run_id, "time": self.started_at}
        )

    def append_event(self, event: dict[str, object]) -> None:
        with self._lock:
            with open(self.events_path, "a", encoding="utf-8") as handle:
                handle.write(
                    json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n"
                )
                handle.flush()

    def save_request(self, request_id: int, body: object) -> Path:
        with self._lock:
            path = self.requests_dir / f"{request_id:04d}.json"
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(body, handle, ensure_ascii=False, indent=2)
                handle.write("\n")
                handle.flush()
            return path

    def begin_request(
        self, route: str, body: dict[str, object]
    ) -> dict[str, object]:
        with self._lock:
            request_id = len(self.requests) + 1
            # Every already-begun request is a lineage candidate; analysis is
            # decided at begin time and usage is completed later.
            candidates = list(self.requests.values())
            predecessor, kind = select_predecessor(candidates, body, route)

            if predecessor is not None:
                sequence_id = str(predecessor["sequence_id"])
                sequence_position = len(self.sequences[sequence_id]["request_ids"])
            else:
                sequence_id = f"seq-{len(self.sequences) + 1}"
                self.sequences[sequence_id] = {
                    "sequence_id": sequence_id,
                    "request_ids": [],
                    "created_at": now_iso(),
                }
                sequence_position = 0

            analysis = analyze_request(
                body, route, predecessor, kind, sequence_id, sequence_position
            )
            request_path = self.save_request(request_id, body)

            record: dict[str, object] = {
                "request_id": request_id,
                "route": route,
                "sequence_id": sequence_id,
                "sequence_position": sequence_position,
                "predecessor_id": analysis["predecessor_id"],
                "predecessor_kind": analysis["predecessor_kind"],
                "prefix_status": analysis["prefix_status"],
                "first_changed_path": analysis["first_changed_path"],
                "changed_paths": analysis["changed_paths"],
                "common_prefix_length": analysis["common_prefix_length"],
                "hash": analysis["hash"],
                "predecessor_hash": analysis["predecessor_hash"],
                "increment": analysis["increment"],
                "tools": analysis["tools"],
                "model": analysis["model"],
                "user_id": analysis["user_id"],
                "request_path": str(request_path),
                "started_at": now_iso(),
                "finished_at": None,
                "duration_ms": None,
                "usage": None,
                "usage_status": "missing",
                "derived": None,
                "stats": None,
                "spike": False,
                "forward": None,
                "status": "started",
                "body": body,
            }
            self.requests[request_id] = record
            self.sequences[sequence_id]["request_ids"].append(request_id)

            self.append_event(
                {
                    "event": "request_received",
                    "request_id": request_id,
                    "route": route,
                    "time": now_iso(),
                }
            )
            self.append_event(
                {
                    "event": "request_analyzed",
                    "request_id": request_id,
                    "sequence_id": sequence_id,
                    "predecessor_id": record["predecessor_id"],
                    "prefix_status": record["prefix_status"],
                    "time": now_iso(),
                }
            )
            return record

    def finish_request(
        self,
        request_id: int,
        usage: Optional[dict[str, object]],
        forward: Optional[dict[str, object]],
    ) -> None:
        """Finalize one request: merge usage, compute per-sequence statistics
        and spike state, then rewrite the report atomically."""
        with self._lock:
            record = self.requests[request_id]
            record["usage"] = usage
            record["usage_status"] = "ok" if usage is not None else "missing"
            record["derived"] = derive_usage(usage)
            record["forward"] = forward
            record["finished_at"] = now_iso()
            started = datetime.fromisoformat(str(record["started_at"]))
            finished = datetime.fromisoformat(str(record["finished_at"]))
            record["duration_ms"] = round(
                (finished - started).total_seconds() * 1000, 1
            )
            record["status"] = "finished"

            sequence = self.sequences[str(record["sequence_id"])]
            prior_samples: list[float] = []
            for rid in sequence["request_ids"]:
                if int(rid) == request_id:
                    break
                earlier = self.requests[int(rid)]
                earlier_derived = earlier.get("derived")
                if isinstance(earlier_derived, dict):
                    value = earlier_derived.get("non_hit_tokens")
                    if value is not None:
                        prior_samples.append(float(value))

            current_derived = record["derived"]
            current_non_hit: Optional[float] = None
            if isinstance(current_derived, dict):
                value = current_derived.get("non_hit_tokens")
                if value is not None:
                    current_non_hit = float(value)

            samples_including_current = prior_samples[:]
            if current_non_hit is not None:
                samples_including_current.append(current_non_hit)

            record["stats"] = population_stats(samples_including_current)
            record["spike"] = (
                detect_spike(current_non_hit, prior_samples)
                if current_non_hit is not None
                else False
            )

            self.append_event(
                {
                    "event": "request_finished",
                    "request_id": request_id,
                    "prefix_status": record["prefix_status"],
                    "usage_status": record["usage_status"],
                    "spike": record["spike"],
                    "time": now_iso(),
                }
            )
            self.write_report()

    def add_warning(self, message: str, request_id: Optional[int] = None) -> None:
        with self._lock:
            warning: dict[str, object] = {"message": message, "time": now_iso()}
            if request_id is not None:
                warning["request_id"] = request_id
            self.warnings.append(warning)
            self.append_event(
                {"event": "warning", "warning": warning, "time": now_iso()}
            )

    def report_snapshot(self) -> dict[str, object]:
        with self._lock:
            requests = [
                self._request_view(self.requests[rid])
                for rid in sorted(self.requests)
            ]
            sequences = [
                {
                    "sequence_id": seq["sequence_id"],
                    "request_ids": list(seq["request_ids"]),
                    "request_count": len(seq["request_ids"]),
                    "created_at": seq["created_at"],
                }
                for seq in self.sequences.values()
            ]
            return {
                "run": {
                    "id": self.run_id,
                    "started_at": self.started_at,
                    "upstream_base": self.upstream_base,
                    "report_path": str(self.report_path),
                },
                "summary": self._summary(requests),
                "sequences": sequences,
                "requests": requests,
                "tool_summary": self._tool_summary(requests),
                "warnings": list(self.warnings),
            }

    @staticmethod
    def _request_view(record: dict[str, object]) -> dict[str, object]:
        view = dict(record)
        view.pop("body", None)
        return view

    def _summary(self, requests: list[dict[str, object]]) -> dict[str, object]:
        comparable = [r for r in requests if r["predecessor_id"] is not None]
        stable = sum(1 for r in requests if r["prefix_status"] == "stable")
        changed = sum(1 for r in requests if r["prefix_status"] == "changed")
        unknown = sum(1 for r in requests if r["prefix_status"] == "unknown")
        missing_usage = sum(1 for r in requests if r["usage_status"] == "missing")
        spikes = sum(1 for r in requests if r["spike"])

        def total(field: str) -> Optional[float]:
            values = []
            for r in requests:
                derived = r.get("derived")
                if isinstance(derived, dict):
                    value = derived.get(field)
                    if value is not None:
                        values.append(float(value))
            return sum(values) if values else None

        return {
            "request_count": len(requests),
            "sequence_count": len(self.sequences),
            "comparable_count": len(comparable),
            "stable_prefix_count": stable,
            "changed_prefix_count": changed,
            "unknown_prefix_count": unknown,
            "stable_prefix_rate": (
                round(stable / len(comparable), 4) if comparable else None
            ),
            "missing_usage_count": missing_usage,
            "numeric_spike_count": spikes,
            "cache_hit_tokens": total("cache_hit_tokens"),
            "new_cache_tokens": total("new_cache_tokens"),
            "non_hit_tokens": total("non_hit_tokens"),
            "observed_input_tokens": total("observed_input_tokens"),
        }

    def _tool_summary(self, requests: list[dict[str, object]]) -> list[dict[str, object]]:
        groups: dict[str, dict[str, object]] = {}
        for request in requests:
            tools = request.get("tools")
            if not isinstance(tools, list) or not tools:
                continue
            derived = request.get("derived")
            non_hit: Optional[float] = None
            if isinstance(derived, dict):
                value = derived.get("non_hit_tokens")
                if value is not None:
                    non_hit = float(value)
            for tool in tools:
                group = groups.setdefault(
                    str(tool),
                    {
                        "tool": str(tool),
                        "request_count": 0,
                        "non_hit_samples": [],
                        "total_new_cache_tokens": 0.0,
                        "total_cache_hit_tokens": 0.0,
                    },
                )
                group["request_count"] = int(group["request_count"]) + 1
                if non_hit is not None:
                    group["non_hit_samples"].append(non_hit)
                if isinstance(derived, dict):
                    new_cache = derived.get("new_cache_tokens")
                    if new_cache is not None:
                        group["total_new_cache_tokens"] += float(new_cache)
                    cache_hit = derived.get("cache_hit_tokens")
                    if cache_hit is not None:
                        group["total_cache_hit_tokens"] += float(cache_hit)

        result: list[dict[str, object]] = []
        for tool in sorted(groups):
            group = groups[tool]
            samples = group.pop("non_hit_samples")
            stats = population_stats([float(s) for s in samples])
            result.append(
                {
                    "tool": group["tool"],
                    "request_count": group["request_count"],
                    "total_cache_hit_tokens": group["total_cache_hit_tokens"],
                    "total_new_cache_tokens": group["total_new_cache_tokens"],
                    "total_non_hit_tokens": stats["mean"] * stats["n"]
                    if stats["mean"] is not None
                    else None,
                    "average_non_hit_tokens": stats["mean"],
                    "variance": stats["variance"],
                    "stdev": stats["stdev"],
                }
            )
        return result

    def write_report(self) -> None:
        with self._lock:
            snapshot = self.report_snapshot()
            temporary = self.report_path.with_suffix(".json.tmp")
            with open(temporary, "w", encoding="utf-8") as handle:
                json.dump(snapshot, handle, ensure_ascii=False, indent=2)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.report_path)


# ---------------------------------------------------------------------------
# Streaming forwarding
# ---------------------------------------------------------------------------


class StreamEventParser:
    """Incremental SSE side-copy parser.

    Recognizes `event:` and `data:` fields. Parsing failures never interrupt
    forwarding; they are counted and recorded as warnings without fabricating
    usage.
    """

    def __init__(self, store: "RunStore", request_id: int) -> None:
        self.store = store
        self.request_id = request_id
        self._buffer = b""
        self._event_name: Optional[str] = None
        self._data_lines: list[bytes] = []
        self.usage: Optional[dict[str, object]] = None
        self.malformed_events = 0

    def feed(self, chunk: bytes) -> None:
        self._buffer += chunk
        while True:
            index = self._buffer.find(b"\n")
            if index == -1:
                break
            line = self._buffer[:index]
            self._buffer = self._buffer[index + 1:]
            self._handle_line(line.rstrip(b"\r"))

    def _handle_line(self, line: bytes) -> None:
        if line == b"":
            self._dispatch()
            return
        if line.startswith(b"event:"):
            self._event_name = line[len(b"event:"):].strip().decode("utf-8", "replace")
        elif line.startswith(b"data:"):
            self._data_lines.append(line[len(b"data:"):].strip())

    def _dispatch(self) -> None:
        if self._event_name is not None and self._data_lines:
            raw = b"\n".join(self._data_lines)
            try:
                payload = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                self.malformed_events += 1
                self.store.add_warning(
                    "malformed streamed event data for request "
                    f"{self.request_id} (event {self._event_name!r}); "
                    "forwarding continues without fabricating usage",
                    request_id=self.request_id,
                )
            else:
                self._handle_payload(self._event_name, payload)
        self._event_name = None
        self._data_lines = []

    def _handle_payload(self, event_name: str, payload: object) -> None:
        if not isinstance(payload, dict):
            return
        usage: Optional[object] = None
        if event_name == "message_start":
            message = payload.get("message")
            if isinstance(message, dict):
                usage = message.get("usage")
        elif event_name == "message_delta":
            usage = payload.get("usage")
        if not isinstance(usage, dict):
            return
        self.usage = merge_usage_event(self.usage, usage)
        self.store.append_event(
            {
                "event": "usage_observed",
                "request_id": self.request_id,
                "usage": dict(usage),
                "time": now_iso(),
            }
        )


class ProbeServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        server_address: tuple[str, int],
        store: "RunStore",
        upstream_base: str,
    ) -> None:
        self.store = store
        self.upstream_base = upstream_base
        super().__init__(server_address, ProbeHandler)


class ProbeHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        pass

    # -- forwarding ---------------------------------------------------------

    def do_POST(self) -> None:
        store: RunStore = self.server.store
        path = urlsplit(self.path).path
        if path != "/messages":
            self._respond(404, "not found")
            return

        content_length = self.headers.get("Content-Length")
        try:
            length = int(content_length) if content_length is not None else -1
        except ValueError:
            length = -1
        if length < 0:
            self._respond(411, "length required")
            return
        if length > MAX_REQUEST_BYTES:
            self._respond(413, "request too large")
            return

        body_bytes = self.rfile.read(length)
        try:
            body = json.loads(body_bytes.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            store.append_event(
                {
                    "event": "invalid_request",
                    "route": path,
                    "error": "invalid JSON body",
                    "bytes": length,
                    "time": now_iso(),
                }
            )
            self._respond(400, "invalid JSON")
            return
        if not isinstance(body, dict):
            store.append_event(
                {
                    "event": "invalid_request",
                    "route": path,
                    "error": "body is not a JSON object",
                    "bytes": length,
                    "time": now_iso(),
                }
            )
            self._respond(400, "body must be a JSON object")
            return

        record = store.begin_request(path, body)
        request_id = int(record["request_id"])
        store.append_event(
            {
                "event": "request_forwarded",
                "request_id": request_id,
                "time": now_iso(),
            }
        )

        # Build the upstream request. Authentication headers are forwarded in
        # memory only and never persisted anywhere.
        upstream = urlsplit(self.server.upstream_base)
        scheme = upstream.scheme.lower()
        if scheme not in ("http", "https"):
            store.finish_request(
                request_id,
                None,
                {"error": f"unsupported upstream scheme {scheme!r}", "status": None},
            )
            self._respond(502, "bad upstream base URL")
            return
        host = upstream.hostname or ""
        port = upstream.port or (443 if scheme == "https" else 80)
        headers: dict[str, str] = {}
        for name, value in self.headers.items():
            lowered = name.lower()
            if lowered in HOP_BY_HOP_HEADERS or lowered in ("host", "content-length"):
                continue
            headers[name] = value

        connection_class = (
            http.client.HTTPSConnection if scheme == "https" else http.client.HTTPConnection
        )
        connection = connection_class(host, port)
        try:
            connection.request("POST", self.path, body=body_bytes, headers=headers)
        except OSError as exc:
            store.finish_request(
                request_id,
                None,
                {"error": f"upstream connection failed: {exc}", "status": None},
            )
            store.append_event(
                {
                    "event": "response_failed",
                    "request_id": request_id,
                    "error": "upstream connection failed",
                    "time": now_iso(),
                }
            )
            connection.close()
            self._respond(502, "upstream connection failed")
            return

        try:
            response = connection.getresponse()
        except OSError as exc:
            store.finish_request(
                request_id,
                None,
                {"error": f"upstream read failed: {exc}", "status": None},
            )
            store.append_event(
                {
                    "event": "response_failed",
                    "request_id": request_id,
                    "error": "upstream read failed",
                    "time": now_iso(),
                }
            )
            connection.close()
            self._respond(502, "upstream read failed")
            return

        # Relay the upstream status and end-to-end response headers.
        self.send_response(response.status)
        content_type = None
        for name, value in response.getheaders():
            lowered = name.lower()
            if lowered in HOP_BY_HOP_HEADERS or lowered in (
                "content-length",
                "connection",
            ):
                continue
            self.send_header(name, value)
            if lowered == "content-type":
                content_type = value
        # Downstream connection close carries the unknown stream length; the
        # complete response is never buffered to invent a Content-Length.
        self.send_header("Connection", "close")
        self.end_headers()

        parser = StreamEventParser(store, request_id)
        total_bytes = 0
        stream_error: Optional[str] = None
        try:
            while True:
                # read1 returns available bytes without waiting for a full
                # buffer, so the first streamed bytes are relayed immediately.
                chunk = response.read1(MAX_STREAM_CHUNK)
                if not chunk:
                    break
                total_bytes += len(chunk)
                self.wfile.write(chunk)
                self.wfile.flush()
                parser.feed(chunk)
        except (ConnectionError, OSError) as exc:
            stream_error = f"downstream disconnect: {exc}"
        finally:
            connection.close()
            self.close_connection = True

        forward = {
            "status": response.status,
            "content_type": content_type,
            "bytes": total_bytes,
            "error": stream_error,
        }
        if parser.malformed_events:
            store.add_warning(
                f"{parser.malformed_events} malformed streamed event(s) for "
                f"request {request_id}; forwarded unchanged",
                request_id=request_id,
            )
        store.finish_request(request_id, parser.usage, forward)
        if stream_error is not None:
            store.append_event(
                {
                    "event": "response_failed",
                    "request_id": request_id,
                    "error": stream_error,
                    "time": now_iso(),
                }
            )
        else:
            store.append_event(
                {
                    "event": "response_completed",
                    "request_id": request_id,
                    "status": response.status,
                    "bytes": total_bytes,
                    "time": now_iso(),
                }
            )

    def do_GET(self) -> None:
        self._respond(404, "not found")

    def _respond(self, status: int, message: str) -> None:
        payload = (message + "\n").encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)
        self.wfile.flush()
        self.close_connection = True


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------


def _check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def _analysis_self_test() -> None:
    """Deterministic analysis assertions (Task 1 scope)."""
    print("self-test: canonical ordering and array strictness")
    _check(
        canonical_bytes({"b": 1, "a": 2}) == canonical_bytes({"a": 2, "b": 1}),
        "object key order must not matter",
    )
    _check(
        canonical_bytes({"x": [1, 2]}) != canonical_bytes({"x": [2, 1]}),
        "array order must matter",
    )
    _check(
        canonical_bytes({"s": "aé"}) == canonical_bytes({"s": "aé"}),
        "unicode bytes must round-trip",
    )

    print("self-test: append-only stability")
    base = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "tools": [{"name": "read", "input_schema": {"type": "object"}}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "hello"}]},
            {
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}]},
        ],
        "metadata": {"user_id": "u1"},
    }
    appended = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "tools": [{"name": "read", "input_schema": {"type": "object"}}],
        "messages": base["messages"] + [
            {"role": "user", "content": [{"type": "text", "text": "more"}]}
        ],
        "metadata": {"user_id": "u1"},
    }
    candidates = [{"request_id": 1, "sequence_id": "seq-1", "body": base, "hash": json_hash(base)}]
    pred, kind = select_predecessor(candidates, appended, "/messages")
    _check(pred is not None and kind == "exact", "append must match exactly")
    analysis = analyze_request(appended, "/messages", pred, kind, "seq-1", 1)
    _check(analysis["prefix_status"] == "stable", "append-only must be stable")
    _check(analysis["common_prefix_length"] == 2, "prefix length must be 2")
    increment = analysis["increment"]
    assert isinstance(increment, dict)
    _check(increment["appended_messages"] == 1, "one appended message")
    _check(increment["user_text_bytes"] == len("more".encode("utf-8")), "user text bytes")

    print("self-test: system and tools changes")
    changed_system = dict(appended)
    changed_system["system"] = [{"type": "text", "text": "sys2"}]
    pred, kind = select_predecessor(candidates, changed_system, "/messages")
    _check(pred is not None and kind == "exact", "system change keeps exact match")
    analysis = analyze_request(changed_system, "/messages", pred, kind, "seq-1", 2)
    _check(analysis["prefix_status"] == "changed", "system change must be changed")
    _check(
        analysis["first_changed_path"] == "$.system[0].text",
        "first changed path is the system text block",
    )
    _check("$.system[0].text" in analysis["changed_paths"], "changed paths include system")

    changed_tools = dict(appended)
    changed_tools["tools"] = [{"name": "write", "input_schema": {"type": "object"}}]
    pred, kind = select_predecessor(candidates, changed_tools, "/messages")
    analysis = analyze_request(changed_tools, "/messages", pred, kind, "seq-1", 3)
    _check(analysis["prefix_status"] == "changed", "tools change must be changed")
    _check(
        analysis["first_changed_path"] == "$.tools[0].name",
        "first changed path is the tool name",
    )

    print("self-test: non-anchor historical mutation")
    mutated = dict(base)
    mutated["messages"] = [
        base["messages"][0],
        {"role": "assistant", "content": [{"type": "text", "text": "rewritten"}]},
    ]
    pred, kind = select_predecessor(candidates, mutated, "/messages")
    _check(pred is not None and kind == "mutation", "anchor must identify mutation")
    analysis = analyze_request(mutated, "/messages", pred, kind, "seq-1", 4)
    _check(analysis["prefix_status"] == "changed", "historical mutation must be changed")
    _check(
        analysis["first_changed_path"] == "$.messages[1].content[0].text",
        "changed path points at the rewritten block",
    )

    print("self-test: interleaved sequences")
    other = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "other-a"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "other-b"}]},
        ],
        "metadata": {"user_id": "u1"},
    }
    other_next = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "messages": other["messages"] + [
            {"role": "user", "content": [{"type": "text", "text": "other-c"}]}
        ],
        "metadata": {"user_id": "u1"},
    }
    base_next = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "tools": [{"name": "read", "input_schema": {"type": "object"}}],
        "messages": base["messages"] + [
            {"role": "user", "content": [{"type": "text", "text": "base-more"}]}
        ],
        "metadata": {"user_id": "u1"},
    }
    interleaved = [
        {"request_id": 1, "sequence_id": "seq-1", "body": base, "hash": json_hash(base)},
        {"request_id": 2, "sequence_id": "seq-2", "body": other, "hash": json_hash(other)},
    ]
    pred, kind = select_predecessor(interleaved, base_next, "/messages")
    _check(pred is not None and int(pred["request_id"]) == 1, "base_next matches seq-1")
    _check(kind == "exact", "base_next exact match")
    pred, kind = select_predecessor(interleaved, other_next, "/messages")
    _check(pred is not None and int(pred["request_id"]) == 2, "other_next matches seq-2")
    _check(kind == "exact", "other_next exact match")

    print("self-test: ambiguous identical anchors stay unknown")
    seq_a = {
        "model": "deepseek-chat",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "same-first"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "a"}]},
        ],
        "metadata": {"user_id": "u1"},
    }
    seq_b = {
        "model": "deepseek-chat",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "same-first"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "b"}]},
        ],
        "metadata": {"user_id": "u1"},
    }
    ambiguous = {
        "model": "deepseek-chat",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "same-first"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "c"}]},
        ],
        "metadata": {"user_id": "u1"},
    }
    two_anchors = [
        {"request_id": 1, "sequence_id": "seq-1", "body": seq_a, "hash": json_hash(seq_a)},
        {"request_id": 2, "sequence_id": "seq-2", "body": seq_b, "hash": json_hash(seq_b)},
    ]
    pred, kind = select_predecessor(two_anchors, ambiguous, "/messages")
    _check(pred is None and kind is None, "two identical anchors must stay unknown")

    print("self-test: tool resolution")
    tool_messages = [
        {
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "tu1", "name": "read", "input": {"path": "a"}}
            ],
        },
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tu1",
                    "content": [{"type": "text", "text": "ok"}],
                }
            ],
        },
    ]
    resolved = resolve_tool_names(tool_messages, ["tu1", "missing"])
    _check(resolved["tu1"] == "read", "tool id resolves to name")
    _check(resolved["missing"] is None, "unresolved id stays unresolved")

    print("self-test: split usage merging")
    usage = None
    usage = merge_usage_event(
        usage,
        {
            "input_tokens": 10,
            "cache_read_input_tokens": 5,
            "cache_creation_input_tokens": 3,
        },
    )
    usage = merge_usage_event(usage, {"output_tokens": 7})
    _check(usage["input_tokens"] == 10, "input tokens merged")
    _check(usage["output_tokens"] == 7, "output tokens merged")
    derived = derive_usage(usage)
    _check(derived["cache_hit_tokens"] == 5, "cache hit derived")
    _check(derived["new_cache_tokens"] == 3, "new cache derived")
    _check(derived["non_hit_tokens"] == 13, "non-hit derived")
    _check(derived["observed_input_tokens"] == 18, "observed input derived")

    usage_only_output = merge_usage_event(None, {"output_tokens": 7})
    derived_partial = derive_usage(usage_only_output)
    _check(
        derived_partial["cache_hit_tokens"] is None
        and derived_partial["new_cache_tokens"] is None,
        "missing cache fields keep derived values null",
    )
    _check(derived_partial["non_hit_tokens"] is None, "missing inputs keep non-hit null")
    _check(derive_usage(None)["cache_hit_tokens"] is None, "no usage keeps null")

    print("self-test: five-sample spike threshold")
    _check(detect_spike(100.0, [10.0, 10.0, 10.0, 10.0]) is False, "four samples: no spike")
    _check(detect_spike(100.0, [10.0] * 5) is True, "zero stdev: above mean is a spike")
    _check(detect_spike(10.0, [10.0] * 5) is False, "equal mean: no spike")
    samples = [10.0, 20.0, 10.0, 20.0, 10.0]
    stats = population_stats(samples)
    threshold = stats["mean"] + 3 * stats["stdev"]
    _check(detect_spike(threshold + 1.0, samples) is True, "above 3 sigma is a spike")
    _check(detect_spike(threshold - 1.0, samples) is False, "below 3 sigma is not a spike")

    print("self-test: atomic report and top-level keys")
    root = Path("target/cache-probe/self-test")
    store = RunStore(root, upstream_base="https://fixture.invalid", run_id="analysis-test")
    first = store.begin_request("/messages", base)
    store.finish_request(first["request_id"], usage, {"status": 200})
    second = store.begin_request("/messages", appended)
    store.finish_request(second["request_id"], usage, {"status": 200})
    _check(store.report_path.exists(), "report.json exists")
    report = json.loads(store.report_path.read_text(encoding="utf-8"))
    _check(tuple(report.keys()) == REPORT_TOP_LEVEL, "report top-level key order")
    _check(report["summary"]["request_count"] == 2, "two requests summarized")
    _check(report["requests"][1]["prefix_status"] == "stable", "second request stable")
    _check(report["requests"][1]["sequence_id"] == "seq-1", "same sequence")
    _check(store.events_path.exists(), "events.jsonl exists")
    events = store.events_path.read_text(encoding="utf-8").strip().splitlines()
    _check(len(events) >= 5, "run events appended")
    store.write_report()
    _check(
        store.report_path.with_suffix(".json.tmp").exists() is False,
        "temporary report file replaced",
    )


class FixtureHandler(BaseHTTPRequestHandler):
    """Deterministic in-process upstream for the self-test."""

    protocol_version = "HTTP/1.1"
    received_auth: Optional[str] = None
    first_sent = threading.Event()
    release = threading.Event()
    fail_requests = 0

    def log_message(self, format: str, *args: object) -> None:
        pass

    EXPECTED_STREAM = (
        b"event: message_start\n"
        b'data: {"type":"message_start","message":{"usage":'
        b'{"input_tokens":10,"cache_read_input_tokens":5,'
        b'"cache_creation_input_tokens":3}}}\n\n'
        b"event: content_block_delta\n"
        b'data: {"type":"content_block_delta","delta":'
        b'{"type":"text_delta","text":"hi"}}\n\n'
        b"event: message_delta\n"
        b'data: {"type":"message_delta","usage":{"output_tokens":7}}\n\n'
    )

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body_bytes = self.rfile.read(length)
        body = json.loads(body_bytes.decode("utf-8"))
        FixtureHandler.received_auth = self.headers.get("x-api-key")
        if isinstance(body, dict) and body.get("fail"):
            FixtureHandler.fail_requests += 1
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        first, _, rest = self.EXPECTED_STREAM.partition(b"\n\n")
        self.wfile.write(first + b"\n\n")
        self.wfile.flush()
        FixtureHandler.first_sent.set()
        FixtureHandler.release.wait(timeout=10)
        self.wfile.write(rest)
        self.wfile.flush()
        self.close_connection = True


def _proxy_self_test() -> None:
    """Deterministic streaming forwarding assertions (Task 2 scope)."""
    print("self-test: streaming fixture upstream")
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    fixture.daemon_threads = True
    fixture_thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    fixture_thread.start()

    root = Path("target/cache-probe/self-test")
    store = RunStore(
        root,
        upstream_base=f"http://127.0.0.1:{fixture.server_address[1]}",
        run_id="proxy-test",
    )
    probe = ProbeServer(
        ("127.0.0.1", 0), store, f"http://127.0.0.1:{fixture.server_address[1]}"
    )
    probe.daemon_threads = True
    probe_thread = threading.Thread(target=probe.serve_forever, daemon=True)
    probe_thread.start()
    probe_port = probe.server_address[1]

    body = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
        "metadata": {"user_id": "u-proxy"},
    }
    fail_body = {
        "model": "deepseek-chat",
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "fail"}]}],
        "metadata": {"user_id": "u-proxy"},
        "fail": True,
    }
    auth_header = "self-test-secret"

    first_chunk_received = threading.Event()
    client_done = threading.Event()
    client_chunks: list[bytes] = []
    client_status: list[int] = []
    client_content_type: list[Optional[str]] = []

    def client_work() -> None:
        connection = http.client.HTTPConnection("127.0.0.1", probe_port)
        connection.request(
            "POST",
            "/messages",
            body=json.dumps(body),
            headers={
                "Content-Type": "application/json",
                "x-api-key": auth_header,
            },
        )
        response = connection.getresponse()
        client_status.append(response.status)
        client_content_type.append(response.getheader("Content-Type"))
        while True:
            chunk = response.read1(MAX_STREAM_CHUNK)
            if not chunk:
                break
            if not client_chunks:
                first_chunk_received.set()
            client_chunks.append(chunk)
        connection.close()
        client_done.set()

    client_thread = threading.Thread(target=client_work, daemon=True)
    client_thread.start()

    print("self-test: first byte before upstream completion")
    _check(
        FixtureHandler.first_sent.wait(timeout=10),
        "fixture flushed its first event",
    )
    _check(
        first_chunk_received.wait(timeout=10),
        "client received the first event while the fixture was still waiting",
    )
    _check(
        not FixtureHandler.release.is_set(),
        "fixture had not completed its stream when the first byte arrived",
    )
    FixtureHandler.release.set()
    _check(client_done.wait(timeout=10), "client finished reading the stream")

    print("self-test: response bytes, status, content type")
    _check(
        b"".join(client_chunks) == FixtureHandler.EXPECTED_STREAM,
        "all streamed bytes unchanged and ordered",
    )
    _check(client_status == [200], "status preserved")
    _check(
        client_content_type == ["text/event-stream"],
        "content type preserved",
    )

    print("self-test: authentication forwarded in memory only")
    _check(
        FixtureHandler.received_auth == auth_header,
        "fixture received the authentication value",
    )

    print("self-test: split usage merged and derived")
    with store._lock:
        first_record = store.requests[1]
        _check(
            first_record["usage"]
            == {
                "input_tokens": 10,
                "cache_read_input_tokens": 5,
                "cache_creation_input_tokens": 3,
                "output_tokens": 7,
            },
            "split usage merged into one request summary",
        )
        derived = first_record["derived"]
        assert isinstance(derived, dict)
        _check(derived["non_hit_tokens"] == 13, "non-hit derived from merged usage")
        _check(derived["cache_hit_tokens"] == 5, "cache-hit derived from merged usage")
        _check(derived["observed_input_tokens"] == 18, "observed input derived")
        _check(
            first_record["forward"]["status"] == 200,
            "forward result recorded",
        )

    print("self-test: upstream failure recorded without retry")
    fail_connection = http.client.HTTPConnection("127.0.0.1", probe_port)
    fail_connection.request(
        "POST",
        "/messages",
        body=json.dumps(fail_body),
        headers={"Content-Type": "application/json", "x-api-key": auth_header},
    )
    fail_response = fail_connection.getresponse()
    fail_response.read()
    fail_connection.close()
    _check(fail_response.status == 503, "upstream status relayed to the client")
    _check(
        FixtureHandler.fail_requests == 1,
        "no retry: fixture saw exactly one failing request",
    )
    with store._lock:
        second_record = store.requests[2]
        _check(
            second_record["forward"]["status"] == 503,
            "forward failure recorded in the report",
        )

    print("self-test: credential redaction")
    secret = auth_header.encode("utf-8")
    header_names = (b"x-api-key", b"authorization")
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if path.name == "proxy-test.json.tmp":
            continue
        content = path.read_bytes()
        _check(
            secret not in content,
            f"authentication value absent from {path.relative_to(root)}",
        )
        for name in header_names:
            _check(
                name not in content,
                f"header name {name.decode()} absent from {path.relative_to(root)}",
            )

    probe.shutdown()
    probe.server_close()
    fixture.shutdown()
    fixture.server_close()


def run_self_test() -> None:
    _analysis_self_test()
    _proxy_self_test()
    print("cache probe self-test: proxy ok")


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="cache_probe",
        description=(
            "Local DeepSeek Anthropic-compatible cache probe: forwarding proxy, "
            "prefix analysis, usage statistics, and dashboard."
        ),
    )
    parser.add_argument(
        "--upstream-base",
        metavar="URL",
        help=(
            "DeepSeek Anthropic-compatible base URL, e.g. "
            "https://api.deepseek.com/anthropic (required in runtime mode)"
        ),
    )
    parser.add_argument(
        "--port",
        type=int,
        default=8787,
        help="local listen port (default 8787)",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("target/cache-probe"),
        help="output root directory (default target/cache-probe)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the deterministic self-test and exit",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        run_self_test()
        return 0

    if not args.upstream_base:
        parser.error("--upstream-base is required in runtime mode")

    upstream = urlsplit(args.upstream_base)
    if upstream.scheme.lower() not in ("http", "https") or not upstream.hostname:
        parser.error("--upstream-base must be an absolute http(s) URL")

    print(
        "WARNING: request bodies are stored locally under the output "
        "directory and may contain private source code, prompts, and tool "
        "output."
    )
    store = RunStore(args.output_root, upstream_base=args.upstream_base)
    server = ProbeServer(
        ("127.0.0.1", args.port), store, args.upstream_base
    )
    host, port = server.server_address[:2]
    print(f"Local proxy: http://127.0.0.1:{port}")
    print(f"Dashboard: http://127.0.0.1:{port}/")
    print(f"Report: {store.report_path}")
    print(f"Requests: {store.requests_dir}")
    print("Press Ctrl+C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
