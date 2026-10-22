"""Deterministic key alignment and estimated QWERTY hand distribution."""

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from difflib import SequenceMatcher
from itertools import zip_longest
from typing import Literal

from model import CONTROL_MASK, MODIFIERS, NativeKey

LEFT_KEYS = frozenset("`12345qwertasdfgzxcvb~!@#$%QWERTASDFGZXCVB")
RIGHT_KEYS = frozenset("67890-=yuiop[]\\hjkl;'nm,./^&*()_+YUIOP{}|HJKL:\"NM<>?")
RIGHT_SPECIAL = frozenset(
    (
        0xFF08,
        0xFFFF,
        0xFF0D,
        0xFF50,
        0xFF57,
        0xFF51,
        0xFF52,
        0xFF53,
        0xFF54,
        0xFF55,
        0xFF56,
    )
)
LEFT_SPECIAL = frozenset((0xFF09, 0xFF1B))  # Tab, Escape on a standard keyboard.
MAX_ALIGNMENT_PRODUCT = 1_000_000
ALIGNMENT_WINDOW = 256
DiffKind = Literal["equal", "replace", "delete", "insert"]


def typing_keys(keys: Iterable[NativeKey]) -> list[NativeKey]:
    return [
        key
        for key in keys
        if key.keyval not in MODIFIERS
        and key.keyval not in (0xFFBF, 0xFFC0)
        and not (key.states & 4 and key.keyval in (ord("c"), ord("d")))
    ]


@dataclass(frozen=True)
class HandCounts:
    left: int
    right: int
    space: int
    unknown: int

    @property
    def left_ratio(self) -> float | None:
        return self.left / (self.left + self.right) if self.left + self.right else None

    @property
    def right_ratio(self) -> float | None:
        return self.right / (self.left + self.right) if self.left + self.right else None


def hand_counts(keys: Iterable[NativeKey]) -> HandCounts:
    left = right = space = unknown = 0
    for key in typing_keys(keys):
        char = chr(key.keyval) if 32 <= key.keyval < 127 else ""
        if key.keyval == 32:
            space += 1
        elif char in LEFT_KEYS or key.keyval in LEFT_SPECIAL:
            left += 1
        elif char in RIGHT_KEYS or key.keyval in RIGHT_SPECIAL:
            right += 1
        else:
            unknown += 1
    return HandCounts(left, right, space, unknown)


@dataclass(frozen=True)
class DiffBlock:
    kind: DiffKind
    actual: tuple[NativeKey, ...]
    reference: tuple[NativeKey, ...]


@dataclass(frozen=True)
class SequenceDiff:
    blocks: tuple[DiffBlock, ...]
    coarse: bool = False

    @property
    def actual_differences(self) -> int:
        return sum(len(block.actual) for block in self.blocks if block.kind != "equal")

    @property
    def reference_differences(self) -> int:
        return sum(
            len(block.reference) for block in self.blocks if block.kind != "equal"
        )


def sequence_diff(
    actual: Sequence[NativeKey], reference: Sequence[NativeKey]
) -> SequenceDiff:
    """Match physical primary keys, including Ctrl/Alt/Super state, not candidates.

    Equal ends are cheap. Large unmatched interiors use bounded windows so long
    repetitive articles cannot trigger an unbounded quadratic whole-text match.
    Every key is retained even when coarse alignment is needed.
    """
    a, b = tuple(actual), tuple(reference)
    signatures_a = tuple((key.keyval, key.states & CONTROL_MASK) for key in a)
    signatures_b = tuple((key.keyval, key.states & CONTROL_MASK) for key in b)
    start = 0
    while start < min(len(a), len(b)) and signatures_a[start] == signatures_b[start]:
        start += 1
    tail = 0
    while (
        tail < min(len(a), len(b)) - start
        and signatures_a[-1 - tail] == signatures_b[-1 - tail]
    ):
        tail += 1
    end_a, end_b = len(a) - tail, len(b) - tail
    blocks: list[DiffBlock] = []
    if start:
        blocks.append(DiffBlock("equal", a[:start], b[:start]))
    coarse = (end_a - start) * (end_b - start) > MAX_ALIGNMENT_PRODUCT
    window = ALIGNMENT_WINDOW if coarse else max(end_a - start, end_b - start, 1)
    ranges_a = range(start, end_a, window)
    ranges_b = range(start, end_b, window)
    for index_a, index_b in zip_longest(ranges_a, ranges_b):
        offset_a = end_a if index_a is None else index_a
        offset_b = end_b if index_b is None else index_b
        stop_a, stop_b = min(offset_a + window, end_a), min(offset_b + window, end_b)
        matcher = SequenceMatcher(
            a=signatures_a[offset_a:stop_a],
            b=signatures_b[offset_b:stop_b],
            autojunk=False,
        )
        for kind, left_a, right_a, left_b, right_b in matcher.get_opcodes():
            blocks.append(
                DiffBlock(
                    kind,
                    a[offset_a + left_a : offset_a + right_a],
                    b[offset_b + left_b : offset_b + right_b],
                )
            )
    if tail:
        blocks.append(DiffBlock("equal", a[end_a:], b[end_b:]))
    return SequenceDiff(tuple(blocks), coarse)
