"""Terminal input and practice lifecycle; Rich owns presentation and refresh."""

from __future__ import annotations

import copy
import json
import os
import select
import shutil
import subprocess
import sys
import termios
import time
import tty
from collections.abc import Iterable
from dataclasses import asdict
from pathlib import Path
from typing import Literal

from analysis import Comparison, compare, completion_keys, completion_status
from events import NativeEvent, parse_native
from model import MODIFIERS, Decoder, Input, NativeState, TypingBuffer
from planning import Planner
from recording import RoundRecorder
from rich.console import Console
from rich.control import Control
from rich.live import Live
from view import screen


class Tail:
    def __init__(self, path: Path):
        self.path, self.offset, self.pending = path, 0, b""

    def read(self) -> list[NativeEvent]:
        with self.path.open("rb") as source:
            source.seek(self.offset)
            data = source.read()
            self.offset = source.tell()
        self.pending += data
        lines = self.pending.split(b"\n")
        self.pending = lines.pop()
        return [parse_native(json.loads(line)) for line in lines if line]


def set_ascii(enabled: bool) -> None:
    result = subprocess.run(
        [
            "qdbus6",
            "org.fcitx.Fcitx5",
            "/rime",
            "org.fcitx.Fcitx.Rime1.SetAsciiMode",
            str(enabled).lower(),
        ],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    if result.returncode:
        raise RuntimeError("切换跟打输入状态失败：" + result.stderr.strip())


def consume_practice_actions(
    buffer: TypingBuffer, actions: Iterable[Input]
) -> Literal["continue", "complete", "quit"]:
    for action in actions:
        if buffer.text == buffer.target:
            return "complete"
        if action.kind == "finish":
            return "quit"
        if action.kind in ("insert", "paste"):
            buffer.insert(action.text, action.kind == "paste")
        elif action.kind == "backspace":
            buffer.delete()
        elif action.kind == "delete":
            buffer.delete(False)
        elif action.kind == "delete_word":
            buffer.delete(word=True)
        elif action.kind == "left":
            buffer.cursor = max(0, buffer.cursor - 1)
        elif action.kind == "right":
            buffer.cursor = min(len(buffer.text), buffer.cursor + 1)
        elif action.kind == "home":
            buffer.cursor = 0
        elif action.kind == "end":
            buffer.cursor = len(buffer.text)
    return "complete" if buffer.text == buffer.target else "continue"


def enable_raw_input(fd: int) -> None:
    """Keep raw input while preserving the newline output Rich expects."""
    tty.setraw(fd)
    attributes = termios.tcgetattr(fd)
    attributes[1] |= termios.OPOST | termios.ONLCR
    termios.tcsetattr(fd, termios.TCSANOW, attributes)


def run(session: Path) -> None:
    spec = json.loads((session / "session.json").read_text())
    buffer, native, decoder = TypingBuffer(spec["target"]), NativeState(), Decoder()
    tail = Tail(session / "events.jsonl")
    planner = Planner(session)
    recorder = RoundRecorder(
        Path(spec["log_directory"]),
        spec.get("source_path"),
        "automation" if spec.get("test_events") else "user",
    )
    round_log: Path | None = None
    recorded_plan = None
    fd = sys.stdin.fileno()
    if not os.isatty(fd):
        raise RuntimeError("trainer requires an interactive terminal")
    saved = termios.tcgetattr(fd)
    frozen: NativeState | None = None
    comparison: Comparison | None = None
    scroll = 0
    raw = False
    display: Live | None = None
    # Source correctness is encoded by color, even if the launching shell set
    # NO_COLOR for command logs. Konsole still supplies its own palette/font.
    console = Console(no_color=False)
    try:
        planner.start(buffer.target)
        (session / "worker-ready").touch()
        print("正在准备输入法…", flush=True)
        deadline = time.monotonic() + 120
        while not (session / "ime-ready").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("输入法初始化超时")
            time.sleep(0.05)
        for event in tail.read():
            native.accept(event)
        if native.keys:
            native.errors.append("准备完成前收到按键，本次不比较效率")
        native.keys.clear()
        termios.tcflush(fd, termios.TCIFLUSH)
        raw = True
        enable_raw_input(fd)
        display = Live(
            console=console,
            screen=True,
            auto_refresh=False,
            redirect_stdout=False,
            redirect_stderr=False,
        )
        display.start()
        sys.stdout.write("\x1b[?2004h")
        finished = False
        last_live = 0.0
        while not finished:
            for event in tail.read():
                native.accept(event)
                if (
                    event["event"] == "key"
                    and not event["release"]
                    and buffer.started_ns is None
                    and event["keyval"] not in MODIFIERS
                    and event["keyval"] not in (0xFFBF, 0xFFC0)
                    and not event["states"] & 4
                ):
                    buffer.started_ns = event.get("monotonic_ns", time.monotonic_ns())
            if frozen is None:
                completion = completion_status(buffer, native, time.monotonic_ns())
                if completion != "pending":
                    if completion == "incomplete":
                        native.errors.append(
                            "按键上屏记录与正文未完整对应，本次不比较效率"
                        )
                    frozen = copy.deepcopy(native)
                    prefix = completion_keys(buffer, native)
                    if prefix is not None:
                        frozen.keys = {key.key_id: copy.deepcopy(key) for key in prefix}
                    set_ascii(False)
            observed = frozen or native
            plan = planner.poll()
            if frozen is not None and plan is not recorded_plan:
                round_log = recorder.save(buffer, frozen, plan, round_log)
                recorded_plan = plan
            if (
                frozen is not None
                and comparison is None
                and plan.references is not None
            ):
                comparison = compare(buffer, observed, plan.references)
            frame = screen(
                buffer,
                observed,
                plan,
                shutil.get_terminal_size((100, 30)),
                comparison,
                scroll,
                source_path=spec.get("source_path"),
            )
            scroll = min(scroll, frame.max_scroll)
            display.update(frame.renderable, refresh=True)
            if frame.caret is not None:
                row, column = frame.caret
                console.control(Control.move_to(column - 1, row - 1))
            if spec.get("test_events") and time.monotonic() - last_live > 0.1:
                live = {
                    "text": buffer.text,
                    "errors": buffer.errors,
                    "completed": frozen is not None,
                    "deleted_characters": buffer.deleted_characters,
                    "counters": asdict(observed.counters()),
                    "reference_hidden": frozen is None,
                    "plan_status": plan.status,
                    "shortest_valid": comparison.shortest.valid
                    if comparison
                    else False,
                    "fewest_valid": comparison.fewest.valid if comparison else False,
                    "source_aligned_shortest": bool(
                        comparison
                        and comparison.shortest.source
                        and not comparison.shortest.source.error
                    ),
                    "source_aligned_fewest": bool(
                        comparison
                        and comparison.fewest.source
                        and not comparison.fewest.source.error
                    ),
                    "extra_keys_shortest": comparison.shortest.extra
                    if comparison
                    else None,
                    "extra_keys_fewest": comparison.fewest.extra
                    if comparison
                    else None,
                    "findings": len(comparison.shortest.explanation.findings)
                    + len(comparison.fewest.explanation.findings)
                    if comparison
                    else 0,
                }
                (session / "live.tmp").write_text(json.dumps(live, ensure_ascii=False))
                (session / "live.tmp").replace(session / "live.json")
                last_live = time.monotonic()
            if not select.select([fd], [], [], 0.05)[0]:
                continue
            data = os.read(fd, 4096)
            if not data:
                break
            actions = decoder.feed(data)
            if frozen is None:
                finished = consume_practice_actions(buffer, actions) == "quit"
                continue
            for action in actions:
                if action.kind == "finish":
                    finished = True
                    break
                if frozen is not None:
                    if action.kind == "home":
                        scroll = 0
                    elif action.kind == "end":
                        scroll = frame.max_scroll
                    elif action.kind in ("up", "page_up"):
                        scroll = max(
                            0,
                            scroll
                            - (frame.page_step if action.kind == "page_up" else 1),
                        )
                    elif action.kind in ("down", "page_down"):
                        scroll = min(
                            frame.max_scroll,
                            scroll
                            + (frame.page_step if action.kind == "page_down" else 1),
                        )
                    elif (
                        action.kind == "retry"
                        and comparison is not None
                        and comparison.retry_text
                    ):
                        target = comparison.retry_text
                        set_ascii(False)
                        buffer, native = TypingBuffer(target), NativeState()
                        frozen, comparison = None, None
                        round_log, recorded_plan = None, None
                        scroll = 0
                        planner.start(target)
                    continue
    except Exception as error:
        (session / "worker-failure").write_text(str(error))
        raise
    finally:
        try:
            planner.close()
        finally:
            if raw:
                try:
                    if display is not None:
                        display.stop()
                finally:
                    termios.tcsetattr(fd, termios.TCSADRAIN, saved)
                    sys.stdout.write("\x1b[?2004l")
                    sys.stdout.flush()
    print(f"已退出，完成轮次日志：{recorder.directory}", flush=True)
