"""Explain operation costs using commit spans and actual buffer edits, without NLP."""

from collections.abc import Iterable
from dataclasses import dataclass
from itertools import pairwise
from typing import Literal

from keystrokes import HandCounts, SequenceDiff, hand_counts, sequence_diff, typing_keys
from model import (
    BACKSPACE,
    CONTROL_MASK,
    DELETE,
    Counters,
    NativeKey,
    NativeState,
    TypingBuffer,
)
from solver import ParagraphReference, References

CAPTURE_SETTLE_NS = 500_000_000


def inserted_text(buffer: TypingBuffer) -> str:
    return "".join(edit["text"] for edit in buffer.edits if edit["event"] == "insert")


def counts(keys: Iterable[NativeKey]) -> Counters:
    state = NativeState()
    state.keys = {key.key_id: key for key in keys}
    return state.counters()


def reference_keys(reference: ParagraphReference) -> list[NativeKey]:
    result = []
    for index, event in enumerate(reference["trace"]):
        commit = event["commit"]
        result.append(
            NativeKey(
                str(index),
                event["key"],
                0,
                event["input_before"],
                event["handled"],
                commit,
            )
        )
    return result


@dataclass(frozen=True)
class CommitGroup:
    text: str
    keys: tuple[NativeKey, ...]


def groups(keys: Iterable[NativeKey]) -> tuple[list[CommitGroup], list[NativeKey]]:
    output: list[CommitGroup] = []
    pending: list[NativeKey] = []
    for key in typing_keys(keys):
        pending.append(key)
        commit = key.commit
        if key.handled is False:
            if 32 <= key.keyval < 127 and not key.states & CONTROL_MASK:
                commit += chr(key.keyval)
            elif key.keyval == 0xFF0D:
                commit += "\n"
            elif key.keyval == 0xFF09:
                commit += "\t"
        if commit:
            output.append(CommitGroup(commit, tuple(pending)))
            pending = []
    return output, pending


def completion_keys(
    buffer: TypingBuffer, native: NativeState
) -> list[NativeKey] | None:
    """Return the physical prefix containing all accepted inserts and deletions."""
    expected = inserted_text(buffer)
    commits, _ = groups(native.keys.values())
    outputs = {group.keys[-1].key_id: group.text for group in commits}
    output = ""
    deletions = 0
    prefix = []
    for key in native.keys.values():
        prefix.append(key)
        output += outputs.get(key.key_id, "")
        if key.handled is False and (
            key.keyval in (BACKSPACE, DELETE)
            or (key.keyval == ord("w") and key.states & 4)
        ):
            deletions += 1
        if not expected.startswith(output) or deletions > buffer.deletion_actions:
            break
        if output == expected and deletions == buffer.deletion_actions:
            return prefix
    return None


def completion_status(
    buffer: TypingBuffer, native: NativeState, now_ns: int
) -> Literal["pending", "ready", "incomplete"]:
    if buffer.text != buffer.target:
        return "pending"
    if buffer.pasted_characters or native.errors:
        return "ready"  # Already excluded from efficiency comparison.
    prefix = completion_keys(buffer, native)
    if prefix is not None and not counts(prefix).pending_replies:
        return "ready"
    if (
        buffer.completed_ns is not None
        and now_ns - buffer.completed_ns >= CAPTURE_SETTLE_NS
    ):
        return "incomplete"
    return "pending"


@dataclass(frozen=True)
class Finding:
    text: str
    actual: tuple[NativeKey, ...]
    reference: tuple[NativeKey, ...]
    extra: int
    reasons: tuple[str, ...]


@dataclass(frozen=True)
class Explanation:
    findings: tuple[Finding, ...]
    correction_keys: int
    note: str


@dataclass(frozen=True)
class SourceToken:
    text: str
    actual: tuple[NativeKey, ...]
    reference: tuple[NativeKey, ...]
    actual_commits: int
    reference_commits: int
    diff: SequenceDiff


@dataclass(frozen=True)
class SourceAlignment:
    tokens: tuple[SourceToken, ...]
    corrections: tuple[NativeKey, ...]
    error: str | None = None


def _carries_code(previous: CommitGroup, following: CommitGroup) -> bool:
    """A top-screen key both commits the previous text and starts the next code."""
    key = previous.keys[-1]
    return bool(
        key.handled
        and key.before
        and not key.states & CONTROL_MASK
        and (
            65 <= key.keyval <= 90
            or 97 <= key.keyval <= 122
            or key.keyval in map(ord, ";`")
        )
        and following.keys[0].before == chr(key.keyval)
    )


def align_source(
    buffer: TypingBuffer, native: NativeState, reference: ParagraphReference
) -> SourceAlignment:
    actual_groups, tail = groups(native.keys.values())
    reference_groups, reference_tail = groups(reference_keys(reference))
    inserted = inserted_text(buffer)
    if (
        "".join(group.text for group in actual_groups) != inserted
        or "".join(group.text for group in reference_groups) != buffer.target
        or reference_tail
    ):
        return SourceAlignment((), (), "上屏记录未对齐")
    origins = [i for i, group in enumerate(actual_groups) for _ in group.text]
    final: list[int] = []
    consumed = 0
    for edit in buffer.edits:
        start = edit["at"]
        if edit["event"] == "insert":
            size = len(edit["text"])
            final[start:start] = origins[consumed : consumed + size]
            consumed += size
        else:
            del final[start : start + len(edit["removed"])]
    if len(final) != len(buffer.target):
        return SourceAlignment((), (), "编辑记录未对齐")
    spans: list[tuple[int, int, CommitGroup]] = []
    offset = 0
    for group in reference_groups:
        spans.append((offset, offset + len(group.text), group))
        offset += len(group.text)
    boundaries = {0, len(buffer.target), *(end for _, end, _ in spans)}
    for index, (previous, following) in enumerate(pairwise(reference_groups)):
        if _carries_code(previous, following):
            boundaries.discard(spans[index][1])
    # Merge reference commits when an actual commit crosses their boundary;
    # each physical key is then attributed once, even with different word splits.
    intervals: dict[int, tuple[int, int]] = {}
    for position, origin in enumerate(final):
        intervals[origin] = (intervals.get(origin, (position, position))[0], position)
    spans_to_merge = list(intervals.values())
    for index, (previous, following) in enumerate(pairwise(actual_groups)):
        left, right = intervals.get(index), intervals.get(index + 1)
        if (
            left is not None
            and right is not None
            and left[1] + 1 == right[0]
            and _carries_code(previous, following)
        ):
            spans_to_merge.append((left[0], right[1]))
    spans_to_merge.sort()
    crossed = -1
    index = 0
    for point in sorted(boundaries):
        while index < len(spans_to_merge) and spans_to_merge[index][0] < point:
            crossed = max(crossed, spans_to_merge[index][1])
            index += 1
        if point <= crossed:
            boundaries.remove(point)
    ordered = sorted(boundaries)
    tokens = []
    for start, end in pairwise(ordered):
        selected = sorted(set(final[start:end]))
        actual = tuple(key for i in selected for key in actual_groups[i].keys)
        expected = tuple(
            key
            for left, right, group in spans
            if start <= left and right <= end
            for key in group.keys
        )
        tokens.append(
            SourceToken(
                buffer.target[start:end],
                actual,
                expected,
                len(selected),
                sum(start <= left and right <= end for left, right, _ in spans),
                sequence_diff(actual, expected),
            )
        )
    discarded = set(range(len(actual_groups))) - set(final)
    corrections = tuple(
        key
        for i, group in enumerate(actual_groups)
        if i in discarded
        for key in group.keys
    ) + tuple(tail)
    return SourceAlignment(tuple(tokens), corrections)


def explain_alignment(alignment: SourceAlignment) -> Explanation:
    if alignment.error:
        return Explanation((), 0, alignment.error)
    findings = []
    for item in alignment.tokens:
        actual, expected = item.actual, item.reference
        a, b = counts(actual), counts(expected)
        extra = a.total_keys - b.total_keys
        reasons = []
        for difference, label in (
            (a.code_keys - b.code_keys, "码键"),
            (a.confirmations - b.confirmations, "确认"),
            (
                a.second_selection
                + a.third_selection
                - b.second_selection
                - b.third_selection,
                "选重",
            ),
            (a.ime_backspaces, "输入法退格"),
            (a.text_delete_keys, "正文删除键"),
        ):
            if difference > 0:
                reasons.append(f"{label} +{difference}")
        if item.actual_commits > item.reference_commits:
            reasons.append("分次上屏较多")
        if extra != 0 or reasons:
            findings.append(Finding(item.text, actual, expected, extra, tuple(reasons)))
    return Explanation(
        tuple(sorted(findings, key=lambda row: row.extra, reverse=True)),
        len(alignment.corrections),
        "按上屏边界对齐；顶屏键可能开启下一词。已删除的错误上屏另计，不作语义断词",
    )


def explain(
    buffer: TypingBuffer, native: NativeState, reference: ParagraphReference
) -> Explanation:
    return explain_alignment(align_source(buffer, native, reference))


@dataclass(frozen=True)
class TypingMetrics:
    counts: Counters
    keys: tuple[NativeKey, ...]
    elapsed_seconds: float
    keys_per_second: float | None
    keys_per_minute: float | None
    characters_per_minute: float | None
    keys_per_character: float | None
    codes_per_character: float | None
    hands: HandCounts
    capture_valid: bool


@dataclass(frozen=True)
class ReferenceComparison:
    valid: bool
    counts: Counters
    hands: HandCounts
    keys: tuple[NativeKey, ...]
    extra: int | None
    diff: SequenceDiff | None
    explanation: Explanation
    source: SourceAlignment | None


@dataclass(frozen=True)
class Comparison:
    metrics: TypingMetrics
    shortest: ReferenceComparison
    fewest: ReferenceComparison
    retry_text: str | None


def typing_metrics(buffer: TypingBuffer, native: NativeState) -> TypingMetrics:
    actual = native.counters()
    actual_keys = tuple(typing_keys(native.keys.values()))
    capture_valid = (
        buffer.text == buffer.target
        and not buffer.pasted_characters
        and not native.errors
        and not actual.pending_replies
        and len(native.contexts) == 1
    )
    elapsed, characters = buffer.elapsed_seconds, len(buffer.target)
    return TypingMetrics(
        actual,
        actual_keys,
        elapsed,
        actual.total_keys / elapsed if capture_valid and elapsed > 0 else None,
        actual.total_keys * 60 / elapsed if capture_valid and elapsed > 0 else None,
        characters * 60 / elapsed if capture_valid and elapsed > 0 else None,
        actual.total_keys / characters if capture_valid and characters else None,
        actual.code_keys / characters if capture_valid and characters else None,
        hand_counts(actual_keys),
        capture_valid,
    )


def compare(
    buffer: TypingBuffer, native: NativeState, references: References
) -> Comparison:
    metrics = typing_metrics(buffer, native)

    def compare_reference(reference: ParagraphReference) -> ReferenceComparison:
        keys = tuple(typing_keys(reference_keys(reference)))
        counters = counts(keys)
        valid = metrics.capture_valid and reference["status"] != "unavailable"
        source = align_source(buffer, native, reference) if valid else None
        return ReferenceComparison(
            valid,
            counters,
            hand_counts(keys),
            keys,
            metrics.counts.total_keys - counters.total_keys if valid else None,
            sequence_diff(metrics.keys, keys) if valid else None,
            explain_alignment(source)
            if source is not None
            else Explanation((), 0, "未完成、粘贴、采集异常或参考不可用时不比较"),
            source,
        )

    shortest = compare_reference(references["shortest"])
    fewest = compare_reference(references["fewest_selections"])
    findings = [
        row
        for pair in (shortest, fewest)
        for row in pair.explanation.findings
        if row.extra > 0
    ]
    retry = max(findings, key=lambda row: row.extra).text if findings else None
    return Comparison(metrics, shortest, fewest, retry)
