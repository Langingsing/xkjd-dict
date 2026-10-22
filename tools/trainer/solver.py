"""A* over complete dictionary-code actions, validated against the Rime oracle."""

from __future__ import annotations

import heapq
import itertools
import json
import re
import sys
import time
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Protocol, TextIO, TypedDict

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "rime-sim"))
from simulate import SELECT_KEYS, Engine, KeyEvent, State, source_codes

ReferenceStatus = Literal["optimal_in_scope", "feasible_limit", "unavailable"]
Objective = Literal["keys", "selections"]
Score = tuple[int, int]
RAW_COMMIT_KEY = 0xFF0D


class References(TypedDict):
    shortest: ParagraphReference
    fewest_selections: ParagraphReference


class ReferenceData(TypedDict):
    text: str
    keys: list[int]
    status: ReferenceStatus
    expanded: int
    seconds: float
    note: str


class ParagraphReference(TypedDict):
    status: ReferenceStatus
    keys: list[int]
    parts: list[ReferenceData]
    trace: list[KeyEvent]
    note: str


class Oracle(Protocol):
    def run(self, keys: Iterable[int], trace: bool = False) -> State: ...


LITERALS = {
    "，": ",",
    "。": ".",
    "？": "?",
    "！": "!",
    "：": ":",
    "；": ";",
    "、": "\\",
    "（": "(",
    "）": ")",
    "《": "<",
    "》": ">",
    "【": "[",
    "】": "]",
    "“": '"',
    "”": '"',
    "‘": "'",
    "’": "'",
    "\n": "\uff0d",
    "\t": "\uff09",
}


@dataclass(frozen=True)
class Edge:
    end: int
    text: str
    code: str
    literal: bool = False


@dataclass(frozen=True)
class Reference:
    text: str
    keys: tuple[int, ...]
    status: ReferenceStatus
    expanded: int
    seconds: float
    note: str

    def data(self) -> ReferenceData:
        return {
            "text": self.text,
            "keys": list(self.keys),
            "status": self.status,
            "expanded": self.expanded,
            "seconds": self.seconds,
            "note": self.note,
        }


def edges_for(text: str, codes: Mapping[str, set[str]]) -> list[list[Edge]]:
    edges: list[list[Edge]] = [[] for _ in text]
    word_lengths = sorted({len(word) for word in codes if word})
    for start in range(len(text)):
        for length in word_lengths:
            end = start + length
            if end > len(text):
                break
            part = text[start:end]
            for code in sorted(codes.get(part, ())):
                edges[start].append(Edge(end, part, code))
        char = text[start]
        literal = LITERALS.get(char)
        if literal is None and char.isascii() and char.isprintable():
            literal = char
        if literal is not None:
            edges[start].append(Edge(start + 1, char, literal, True))
    return edges


def fingerprint(position: int, state: State) -> tuple[int, str, str, tuple[str, ...]]:
    return position, state.output, state.input, state.menu[:3]


def shortest(
    text: str,
    codes: Mapping[str, set[str]],
    engine: Oracle,
    max_states: int = 10000,
    max_seconds: float = 45,
    objective: Objective = "keys",
) -> Reference:
    started = time.monotonic()
    edges = edges_for(text, codes)
    infinity = 10**9
    lower = [infinity] * len(text) + [0]
    for start in range(len(text) - 1, -1, -1):
        lower[start] = min(
            (len(edge.code) + lower[edge.end] for edge in edges[start]),
            default=infinity,
        )
    if lower[0] >= infinity:
        return Reference(
            text, (), "unavailable", 0, 0, "存在没有可用编码或直接标点输入路径的字符"
        )
    initial = engine.run(())
    serial = itertools.count()

    def score(keys: int, selections: int) -> Score:
        return (keys, selections) if objective == "keys" else (selections, keys)

    queue: list[tuple[Score, Score, int, int, tuple[int, ...], State]] = [
        (score(lower[0], 0), (0, 0), next(serial), 0, (), initial)
    ]
    best = {fingerprint(0, initial): (0, 0)}
    expanded = 0
    feasible: tuple[Score, tuple[int, ...]] | None = None
    evaluations = 0
    limited = False
    while queue:
        _, cost, _, position, keys, state = heapq.heappop(queue)
        if cost != best.get(fingerprint(position, state)):
            continue
        if position == len(text) and state.output == text and not state.input:
            verified = engine.run(keys, trace=True)
            if verified.output != text or verified.input:
                raise RuntimeError("reference final replay mismatch")
            return Reference(
                text,
                keys,
                "optimal_in_scope",
                expanded,
                time.monotonic() - started,
                (
                    "限定范围内按键最少，同键数优先少选重"
                    if objective == "keys"
                    else "限定范围内选重最少，同选重数优先少按键"
                )
                + "；不跨句段组词，不计独立修饰键",
            )
        if expanded >= max_states or time.monotonic() - started > max_seconds:
            break
        expanded += 1
        if position == len(text):
            continue
        for edge in edges[position]:
            # Bound generated work as well as popped states; a wide node must not
            # bypass the time/memory budget while enumerating outgoing edges.
            if (
                time.monotonic() - started > max_seconds
                or evaluations >= max(64, max_states * 4)
                or len(best) >= max(64, max_states * 4)
            ):
                limited = True
                break
            evaluations += 1
            base_keys = keys + tuple(map(ord, edge.code))
            base = engine.run(base_keys)
            if not text.startswith(base.output) or len(base.output) > edge.end:
                continue
            pending = text[len(base.output) : edge.end]
            variants: list[tuple[tuple[int, ...], State, int]] = []
            if not pending and not base.input:
                variants.append((base_keys, base, 0))
            elif pending:
                complete = base.input in codes.get(pending, ())
                complete_literal = (
                    edge.literal and base.input == edge.code and pending == edge.text
                )
                candidate = pending in base.menu[:3] and (complete or complete_literal)
                # English can share the input buffer with Chinese candidates.
                # Return commits raw input; longer runs may need splitting before
                # automatic Chinese top-up changes their output.
                raw_literal = (
                    edge.literal and bool(base.input) and pending == base.input
                )
                if candidate or raw_literal:
                    variants.append(
                        (base_keys, base, 0)
                    )  # Let the next code/punctuation top this word up.
                    rank = base.menu.index(pending) if candidate else 0
                    confirmations = [ord(SELECT_KEYS[rank])]
                    if raw_literal:
                        confirmations.append(RAW_COMMIT_KEY)
                    for confirmation in confirmations:
                        selected_keys = base_keys + (confirmation,)
                        selected = engine.run(selected_keys)
                        if selected.output == text[: edge.end] and not selected.input:
                            variants.append(
                                (
                                    selected_keys,
                                    selected,
                                    int(rank > 0 and confirmation != RAW_COMMIT_KEY),
                                )
                            )
            for next_keys, next_state, selection in variants:
                selections = cost[1] if objective == "keys" else cost[0]
                next_cost = score(len(next_keys), selections + selection)
                mark = fingerprint(edge.end, next_state)
                if mark in best and next_cost >= best[mark]:
                    continue
                best[mark] = next_cost
                if (
                    edge.end == len(text)
                    and next_state.output == text
                    and not next_state.input
                    and (feasible is None or next_cost < feasible[0])
                ):
                    feasible = (next_cost, next_keys)
                heapq.heappush(
                    queue,
                    (
                        score(len(next_keys) + lower[edge.end], selections + selection),
                        next_cost,
                        next(serial),
                        edge.end,
                        next_keys,
                        next_state,
                    ),
                )
        if limited:
            break
    return Reference(
        text,
        feasible[1] if feasible else (),
        "feasible_limit" if feasible else "unavailable",
        expanded,
        time.monotonic() - started,
        "搜索达到状态/时间上限，未证明最优"
        if queue or limited
        else "规则范围内没有找到可回放路径",
    )


def solve_paragraph(
    text: str,
    root: Path,
    executable: Path,
    log: Path,
    max_states: int = 10000,
    max_seconds: float = 45,
    objective: Objective = "keys",
) -> ParagraphReference:
    # Sentence punctuation is included in its preceding chunk, allowing free punctuation top-up.
    parts = re.findall(r"[^，。！？；：\n]*[，。！？；：\n]|[^，。！？；：\n]+$", text)
    assert "".join(parts) == text
    codes = source_codes(root, "xkjd6", parts, False)
    engine = Engine(executable, root, "xkjd6", False, log)
    try:
        with log.with_suffix(".jsonl").open("a", encoding="utf-8") as diagnostics:
            return solve_parts(
                text,
                parts,
                codes,
                engine,
                max_states,
                max_seconds,
                objective,
                diagnostics=diagnostics,
            )
    finally:
        engine.close()


def solve_parts(
    text: str,
    parts: list[str],
    codes: Mapping[str, set[str]],
    engine: Engine,
    max_states: int,
    max_seconds: float,
    objective: Objective,
    *,
    diagnostics: TextIO | None = None,
) -> ParagraphReference:
    results = []
    for index, part in enumerate(parts, 1):
        engine.cache.clear()
        print(f"计算参考路径 {index}/{len(parts)}（{len(part)} 字符）…", flush=True)
        results.append(
            shortest(part, codes, engine, max_states, max_seconds, objective)
        )
        if diagnostics is not None:
            print(
                json.dumps(
                    {"objective": objective, "part": index, **results[-1].data()},
                    ensure_ascii=False,
                ),
                file=diagnostics,
                flush=True,
            )
    keys = tuple(key for result in results for key in result.keys)
    available = all(result.status != "unavailable" for result in results)
    trace = []
    if available:
        replay = engine.run(keys, trace=True)
        trace = list(replay.trace)
        if replay.output != text or replay.input:
            return {
                "status": "unavailable",
                "keys": [],
                "parts": [r.data() for r in results],
                "trace": [],
                "note": "句段合并回放未通过",
            }
    return {
        "status": "optimal_in_scope"
        if all(r.status == "optimal_in_scope" for r in results)
        else ("feasible_limit" if available else "unavailable"),
        "keys": list(keys) if available else [],
        "parts": [r.data() for r in results],
        "trace": trace,
        "note": ("按键最少" if objective == "keys" else "选重最少，其次按键最少")
        + "；逐句段搜索，不跨标点组词，不做语境读音消歧",
    }


def solve_references(
    text: str,
    root: Path,
    executable: Path,
    log: Path,
    max_states: int = 10000,
    max_seconds: float = 45,
) -> References:
    parts = re.findall(r"[^，。！？；：\n]*[，。！？；：\n]|[^，。！？；：\n]+$", text)
    codes = source_codes(root, "xkjd6", parts, False)
    engine = Engine(executable, root, "xkjd6", False, log)
    try:
        with log.with_suffix(".jsonl").open("a", encoding="utf-8") as diagnostics:
            references: References = {
                "shortest": solve_parts(
                    text,
                    parts,
                    codes,
                    engine,
                    max_states,
                    max_seconds,
                    "keys",
                    diagnostics=diagnostics,
                ),
                "fewest_selections": solve_parts(
                    text,
                    parts,
                    codes,
                    engine,
                    max_states,
                    max_seconds,
                    "selections",
                    diagnostics=diagnostics,
                ),
            }
            log.with_suffix(".references.json").write_text(
                json.dumps(references, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            return references
    finally:
        engine.close()
