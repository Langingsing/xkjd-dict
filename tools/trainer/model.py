"""Typing buffer, terminal input decoding and counters independent of the UI."""

from __future__ import annotations

import codecs
import time
from dataclasses import dataclass
from typing import ClassVar, Literal, TypedDict

from events import NativeEvent

BACKSPACE, DELETE, RETURN = 0xFF08, 0xFFFF, 0xFF0D
MODIFIERS = set(range(0xFFE1, 0xFFEF)) | {0xFE03}
CONTROL_MASK = 4 | 8 | 64


class InsertEdit(TypedDict):
    event: Literal["insert"]
    at: int
    text: str
    pasted: bool
    monotonic_ns: int


class DeleteEdit(TypedDict):
    event: Literal["delete"]
    at: int
    removed: str
    monotonic_ns: int


BufferEdit = InsertEdit | DeleteEdit


@dataclass
class Counters:
    total_keys: int = 0
    code_keys: int = 0
    confirmations: int = 0
    second_selection: int = 0
    third_selection: int = 0
    ime_backspaces: int = 0
    text_delete_keys: int = 0
    direct_keys: int = 0
    other_keys: int = 0
    modifier_keys: int = 0
    pending_replies: int = 0


@dataclass
class NativeKey:
    key_id: str
    keyval: int
    states: int
    before: str
    handled: bool | None = None
    commit: str = ""
    monotonic_ns: int | None = None


class NativeState:
    def __init__(self):
        self.keys: dict[str, NativeKey] = {}
        self.preedit = ""
        self.candidates: list[str] = []
        self.commits: list[str] = []
        self.errors: list[str] = []
        self.contexts: set[str] = set()
        self.active_key: str | None = None

    def accept(self, event: NativeEvent) -> None:
        if "context" in event:
            self.contexts.add(event["context"])
        if event["event"] == "key" and not event["release"]:
            self.keys[event["key_id"]] = NativeKey(
                event["key_id"],
                event["keyval"],
                event["states"],
                self.preedit,
                monotonic_ns=event.get("monotonic_ns"),
            )
            self.active_key = event["key_id"]
        elif event["event"] == "handled" and event["key_id"] in self.keys:
            self.keys[event["key_id"]].handled = event["handled"]
        elif event["event"] == "key" and event["release"]:
            active = self.keys.get(self.active_key or "")
            if active is not None and active.keyval == event["keyval"]:
                self.active_key = None
        elif event["event"] == "key_error" or event["event"] == "capture_error":
            self.errors.append(event["error"])
        elif event["event"] == "preedit" or event["event"] == "ui":
            self.preedit = event["text"]
            if event["event"] == "ui":
                self.candidates = event["candidates"]
        elif event["event"] == "commit":
            self.commits.append(event["text"])
            if self.active_key in self.keys:
                self.keys[self.active_key].commit += event["text"]
            elif event["text"]:
                self.errors.append("上屏未能关联到原生按键，本次不比较效率")

    def counters(self) -> Counters:
        result = Counters()
        for key in self.keys.values():
            value, states = key.keyval, key.states
            if value in MODIFIERS:
                result.modifier_keys += 1
                continue
            # These are trainer commands, not attempts to type the paragraph.
            if value in (0xFFBF, 0xFFC0) or (
                states & 4 and value in (ord("c"), ord("d"))
            ):
                continue
            result.total_keys += 1
            if key.handled is None:
                result.pending_replies += 1
            if value == BACKSPACE and key.handled:
                result.ime_backspaces += 1
            elif value in (BACKSPACE, DELETE) and not key.handled:
                result.text_delete_keys += 1
            elif states & CONTROL_MASK:
                result.other_keys += 1
            elif value in (32, RETURN) and key.handled and key.before:
                result.confirmations += 1
            elif value == ord("'") and key.handled and key.before:
                result.second_selection += 1
            elif value == ord("3") and key.handled and key.before:
                result.third_selection += 1
            elif (
                65 <= value <= 90 or 97 <= value <= 122 or value in map(ord, ";`")
            ) and key.handled:
                result.code_keys += 1
            elif not key.handled and (32 <= value < 127 or value == RETURN):
                result.direct_keys += 1
            else:
                result.other_keys += 1
        return result


class TypingBuffer:
    def __init__(self, target: str):
        self.target = target
        self.text = ""
        self.cursor = 0
        self.deleted_characters = 0
        self.deletion_actions = 0
        self.pasted_characters = 0
        self.error_insertions = 0
        self.edits: list[BufferEdit] = []
        self.started_ns: int | None = None
        self.completed_ns: int | None = None

    def insert(self, text: str, pasted: bool = False) -> None:
        if not text:
            return
        now = time.monotonic_ns()
        if self.started_ns is None:
            self.started_ns = now
        self.error_insertions += sum(
            self.cursor + i >= len(self.target) or char != self.target[self.cursor + i]
            for i, char in enumerate(text)
        )
        self.text = self.text[: self.cursor] + text + self.text[self.cursor :]
        self.cursor += len(text)
        if pasted:
            self.pasted_characters += len(text)
        self.edits.append(
            {
                "event": "insert",
                "at": self.cursor - len(text),
                "text": text,
                "pasted": pasted,
                "monotonic_ns": now,
            }
        )
        self.completed_ns = now if self.text == self.target else None

    def delete(self, backward: bool = True, word: bool = False) -> None:
        start, end = self.cursor, self.cursor
        if backward and start:
            start -= 1
            if word:
                while (
                    start
                    and self.text[start - 1] != "\n"
                    and not self.text[start - 1].isspace()
                ):
                    start -= 1
        elif not backward and end < len(self.text):
            end += 1
        removed = self.text[start:end]
        self.text = self.text[:start] + self.text[end:]
        self.cursor = start
        self.deleted_characters += len(removed)
        self.deletion_actions += 1
        self.completed_ns = time.monotonic_ns() if self.text == self.target else None
        self.edits.append(
            {
                "event": "delete",
                "at": start,
                "removed": removed,
                "monotonic_ns": time.monotonic_ns(),
            }
        )

    @property
    def errors(self) -> int:
        return sum(
            index >= len(self.target) or char != self.target[index]
            for index, char in enumerate(self.text)
        )

    @property
    def elapsed_seconds(self) -> float:
        if self.started_ns is None:
            return 0.0
        end = (
            self.completed_ns if self.completed_ns is not None else time.monotonic_ns()
        )
        return max(0, end - self.started_ns) / 1e9

    def summary(self) -> dict:
        return {
            "text": self.text,
            "target": self.target,
            "completed": self.text == self.target,
            "current_errors": self.errors,
            "error_insertions": self.error_insertions,
            "deleted_characters": self.deleted_characters,
            "deletion_actions": self.deletion_actions,
            "pasted_characters": self.pasted_characters,
            "elapsed_seconds": self.elapsed_seconds,
            "edits": self.edits,
        }


@dataclass(frozen=True)
class Input:
    kind: str
    text: str = ""


class Decoder:
    SEQUENCES: ClassVar[dict[bytes, str]] = {
        b"\x1b[D": "left",
        b"\x1b[C": "right",
        b"\x1b[H": "home",
        b"\x1b[F": "end",
        b"\x1b[3~": "delete",
        b"\x1b[12~": "finish",
        b"\x1bOQ": "finish",
        b"\x1b[13~": "retry",
        b"\x1bOR": "retry",
        b"\x1b[A": "up",
        b"\x1b[B": "down",
        b"\x1b[5~": "page_up",
        b"\x1b[6~": "page_down",
        b"\x1b[200~": "paste_start",
        b"\x1b[201~": "paste_end",
    }

    def __init__(self):
        self.pending = bytearray()
        self.utf8 = codecs.getincrementaldecoder("utf-8")("strict")
        self.pasting = False

    def feed(self, data: bytes) -> list[Input]:
        self.pending.extend(data)
        result = []
        while self.pending:
            if self.pending[0] == 27:
                match = next(
                    (
                        sequence
                        for sequence in self.SEQUENCES
                        if self.pending.startswith(sequence)
                    ),
                    None,
                )
                if match is not None:
                    del self.pending[: len(match)]
                    kind = self.SEQUENCES[match]
                    if kind == "paste_start":
                        self.pasting = True
                    elif kind == "paste_end":
                        self.pasting = False
                    else:
                        result.append(Input(kind))
                    continue
                if any(
                    sequence.startswith(self.pending) for sequence in self.SEQUENCES
                ):
                    break
                if self.pending.startswith(b"\x1b["):
                    end = next(
                        (
                            i
                            for i in range(2, len(self.pending))
                            if 0x40 <= self.pending[i] <= 0x7E
                        ),
                        None,
                    )
                    if end is None and len(self.pending) < 64:
                        break
                    del self.pending[
                        : end + 1 if end is not None else len(self.pending)
                    ]
                    result.append(Input("control"))
                    continue
                if self.pending.startswith(b"\x1bO"):
                    if len(self.pending) < 3:
                        break
                    del self.pending[:3]
                    result.append(Input("control"))
                    continue
                # Never render an unknown control sequence as paragraph text.
                del self.pending[0]
                result.append(Input("unknown_escape"))
                continue
            byte = self.pending.pop(0)
            if not self.pasting and byte in (3, 4):
                result.append(Input("finish"))
            elif not self.pasting and byte in (8, 127):
                result.append(Input("backspace"))
            elif not self.pasting and byte == 23:
                result.append(Input("delete_word"))
            elif byte in (10, 13):
                result.append(Input("paste" if self.pasting else "insert", "\n"))
            elif byte == 9:
                result.append(Input("paste" if self.pasting else "insert", "\t"))
            elif byte < 32:
                result.append(Input("control"))
            else:
                text = self.utf8.decode(bytes([byte]))
                if text:
                    result.append(Input("paste" if self.pasting else "insert", text))
        return result
