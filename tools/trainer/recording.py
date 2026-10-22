"""Private, atomic completed-round logs and validated reconstruction for analysis."""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal

from model import (
    BufferEdit,
    DeleteEdit,
    InsertEdit,
    NativeKey,
    NativeState,
    TypingBuffer,
)
from planning import PlanState
from solver import ParagraphReference, ReferenceData, References

FORMAT_VERSION = 1
Origin = Literal["user", "automation"]


@dataclass(frozen=True)
class RoundRecord:
    version: int
    origin: Origin
    completed_at: str
    source_path: str | None
    target: str
    started_ns: int | None
    completed_ns: int
    edits: list[BufferEdit]
    keys: list[NativeKey]
    contexts: list[str]
    errors: list[str]
    plan: PlanState


class RoundRecorder:
    def __init__(self, directory: Path, source_path: str | None, origin: Origin):
        self.directory, self.source_path = directory, source_path
        self.origin: Origin = origin
        self.index = 0
        self.completed_at: str | None = None
        self.current_path: Path | None = None

    def save(
        self,
        buffer: TypingBuffer,
        native: NativeState,
        plan: PlanState,
        path: Path | None = None,
    ) -> Path:
        if buffer.text != buffer.target or buffer.completed_ns is None:
            raise ValueError("only completed rounds can be recorded")
        if path is None:
            self.index += 1
            path = self.directory / f"round-{self.index:04d}.json"
            if path.exists():
                raise FileExistsError(f"拒绝覆盖已有轮次 {path}")
            self.current_path = path
            self.completed_at = datetime.now(UTC).isoformat()
        elif path != self.current_path:
            raise ValueError(f"只能更新当前轮次 {self.current_path}，收到 {path}")
        assert self.completed_at is not None
        record = RoundRecord(
            FORMAT_VERSION,
            self.origin,
            self.completed_at,
            self.source_path,
            buffer.target,
            buffer.started_ns,
            buffer.completed_ns,
            buffer.edits,
            list(native.keys.values()),
            sorted(native.contexts),
            list(native.errors),
            plan,
        )
        temporary: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=self.directory,
                prefix=".round-",
                delete=False,
            ) as file:
                temporary = Path(file.name)  # NamedTemporaryFile creates mode 0600.
                json.dump(asdict(record), file, ensure_ascii=False)
                file.write("\n")
            os.replace(temporary, path)
        except OSError as error:
            raise RuntimeError(f"保存完成轮次失败 {path}: {error}") from error
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        return path


def _fields(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError("expected a JSON object with string keys")
    return {str(key): item for key, item in value.items()}


def _text(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError("expected a string")
    return value


def _integer(value: object) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError("expected a nonnegative integer")
    return value


def _boolean(value: object) -> bool:
    if not isinstance(value, bool):
        raise TypeError("expected a boolean")
    return value


def _items(value: object) -> list[object]:
    if not isinstance(value, list):
        raise TypeError("expected an array")
    return list(value)


def _nullable_text(value: object) -> str | None:
    return None if value is None else _text(value)


def _nullable_integer(value: object) -> int | None:
    return None if value is None else _integer(value)


def _reference(value: object) -> ParagraphReference:
    data = _fields(value)
    status = data["status"]
    if status not in ("optimal_in_scope", "feasible_limit", "unavailable"):
        raise ValueError("invalid reference status")
    parts: list[ReferenceData] = []
    for raw in _items(data["parts"]):
        part = _fields(raw)
        part_status = part["status"]
        if part_status not in ("optimal_in_scope", "feasible_limit", "unavailable"):
            raise ValueError("invalid reference part status")
        seconds = part["seconds"]
        if (
            isinstance(seconds, bool)
            or not isinstance(seconds, (int, float))
            or seconds < 0
        ):
            raise ValueError("invalid reference seconds")
        parts.append(
            ReferenceData(
                text=_text(part["text"]),
                keys=[_integer(k) for k in _items(part["keys"])],
                status=part_status,
                expanded=_integer(part["expanded"]),
                seconds=seconds,
                note=_text(part["note"]),
            )
        )
    reference = ParagraphReference(
        status=status,
        keys=[_integer(k) for k in _items(data["keys"])],
        parts=parts,
        trace=[],
        note=_text(data["note"]),
    )
    for raw in _items(data["trace"]):
        key = _fields(raw)
        reference["trace"].append(
            {
                "key": _integer(key["key"]),
                "handled": _boolean(key["handled"]),
                "commit": _text(key["commit"]),
                "input_before": _text(key["input_before"]),
            }
        )
    if [event["key"] for event in reference["trace"]] != reference["keys"]:
        raise ValueError("reference keys disagree with trace")
    return reference


@dataclass(frozen=True)
class RecordedRound:
    buffer: TypingBuffer
    native: NativeState
    plan: PlanState
    source_path: str | None
    origin: Origin


def load_round(path: Path) -> RecordedRound:
    """Rebuild the original measured state; never synthesize a user's key path."""
    try:
        data = _fields(json.loads(path.read_text(encoding="utf-8")))
        if _integer(data["version"]) != FORMAT_VERSION:
            raise ValueError("unsupported completed-round version")
        origin = data["origin"]
        if origin not in ("user", "automation"):
            raise ValueError("invalid recording origin")
        buffer = TypingBuffer(_text(data["target"]))
        for raw in _items(data["edits"]):
            edit = _fields(raw)
            at, timestamp = _integer(edit["at"]), _integer(edit["monotonic_ns"])
            if at > len(buffer.text):
                raise ValueError("edit position outside text")
            buffer.cursor = at
            if edit["event"] == "insert":
                text, pasted = _text(edit["text"]), _boolean(edit["pasted"])
                if not text:
                    raise ValueError("empty recorded insertion")
                buffer.insert(text, pasted)
                buffer.edits[-1] = InsertEdit(
                    event="insert",
                    at=at,
                    text=text,
                    pasted=pasted,
                    monotonic_ns=timestamp,
                )
            elif edit["event"] == "delete":
                removed = _text(edit["removed"])
                if buffer.text[at : at + len(removed)] != removed:
                    raise ValueError("removed text disagrees with edit history")
                buffer.text = buffer.text[:at] + buffer.text[at + len(removed) :]
                buffer.deleted_characters += len(removed)
                buffer.deletion_actions += 1
                buffer.edits.append(
                    DeleteEdit(
                        event="delete", at=at, removed=removed, monotonic_ns=timestamp
                    )
                )
            else:
                raise ValueError("invalid edit event")
        if buffer.text != buffer.target:
            raise ValueError("recorded edits do not complete the target")
        buffer.started_ns = _nullable_integer(data["started_ns"])
        buffer.completed_ns = _integer(data["completed_ns"])
        if buffer.started_ns is not None and buffer.started_ns > buffer.completed_ns:
            raise ValueError("completion precedes start")
        native = NativeState()
        for raw in _items(data["keys"]):
            key = _fields(raw)
            handled = None if key["handled"] is None else _boolean(key["handled"])
            item = NativeKey(
                _text(key["key_id"]),
                _integer(key["keyval"]),
                _integer(key["states"]),
                _text(key["before"]),
                handled,
                _text(key["commit"]),
                _nullable_integer(key["monotonic_ns"]),
            )
            if item.key_id in native.keys:
                raise ValueError("duplicate native key id")
            native.keys[item.key_id] = item
        native.contexts = {_text(context) for context in _items(data["contexts"])}
        native.errors = [_text(error) for error in _items(data["errors"])]
        plan_data = _fields(data["plan"])
        status = plan_data["status"]
        if status not in ("pending", "ready", "error"):
            raise ValueError("invalid plan status")
        refs = plan_data["references"]
        references = None
        if refs is not None:
            fields = _fields(refs)
            references = References(
                shortest=_reference(fields["shortest"]),
                fewest_selections=_reference(fields["fewest_selections"]),
            )
        if status == "ready" and references is None:
            raise ValueError("ready plan has no references")
        plan = PlanState(status, references, _nullable_text(plan_data["error"]))
        return RecordedRound(
            buffer, native, plan, _nullable_text(data["source_path"]), origin
        )
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ValueError(f"读取完成轮次失败 {path}: {error}") from error
