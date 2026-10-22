"""Validated native observation records shared by capture and frontend."""

from typing import Literal, NotRequired, TypedDict


class Timed(TypedDict):
    monotonic_ns: NotRequired[int]


class Key(Timed):
    event: Literal["key"]
    key_id: str
    context: NotRequired[str]
    keyval: int
    states: int
    release: bool


class Handled(Timed):
    event: Literal["handled"]
    key_id: str
    handled: bool


class Preedit(Timed):
    event: Literal["preedit"]
    context: str
    text: str


class UI(Timed):
    event: Literal["ui"]
    context: str
    text: str
    candidates: list[str]


class Commit(Timed):
    event: Literal["commit"]
    context: str
    text: str


class Failure(Timed):
    event: Literal["key_error", "capture_error"]
    error: str


class Lifecycle(Timed):
    event: Literal["ready", "stop"]


NativeEvent = Key | Handled | Preedit | UI | Commit | Failure | Lifecycle


def parse_native(raw: object) -> NativeEvent:
    if not isinstance(raw, dict):
        raise TypeError("native event must be an object")

    def string(name: str) -> str:
        value = raw.get(name)
        if not isinstance(value, str):
            raise TypeError(f"native event {name} must be a string")
        return value

    def integer(name: str) -> int:
        value = raw.get(name)
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeError(f"native event {name} must be an integer")
        return value

    def boolean(name: str) -> bool:
        value = raw.get(name)
        if not isinstance(value, bool):
            raise TypeError(f"native event {name} must be a boolean")
        return value

    kind = string("event")
    event: NativeEvent
    if kind == "key":
        event = Key(
            event="key",
            key_id=string("key_id"),
            context=string("context"),
            keyval=integer("keyval"),
            states=integer("states"),
            release=boolean("release"),
        )
    elif kind == "handled":
        event = Handled(
            event="handled", key_id=string("key_id"), handled=boolean("handled")
        )
    elif kind == "preedit":
        event = Preedit(event="preedit", context=string("context"), text=string("text"))
    elif kind == "ui":
        candidates = raw.get("candidates")
        if not isinstance(candidates, list) or not all(
            isinstance(item, str) for item in candidates
        ):
            raise ValueError("native candidates must be strings")
        event = UI(
            event="ui",
            context=string("context"),
            text=string("text"),
            candidates=[str(item) for item in candidates],
        )
    elif kind == "commit":
        event = Commit(event="commit", context=string("context"), text=string("text"))
    elif kind in ("key_error", "capture_error"):
        event = Failure(event=kind, error=string("error"))
    elif kind in ("ready", "stop"):
        event = Lifecycle(event=kind)
    else:
        raise ValueError(f"unknown native event: {kind}")
    event["monotonic_ns"] = integer("monotonic_ns")
    return event
