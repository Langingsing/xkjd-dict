"""Rich layouts for practice and results; terminal input remains owned by the trainer."""

from __future__ import annotations

import os
from collections.abc import Iterable
from dataclasses import dataclass
from io import StringIO
from itertools import zip_longest

from analysis import Comparison, ReferenceComparison, SourceAlignment, typing_metrics
from keystrokes import HandCounts, SequenceDiff
from model import CONTROL_MASK, NativeKey, NativeState, TypingBuffer
from planning import PlanState
from rich.align import Align
from rich.cells import cell_len
from rich.console import Console, Group, RenderableType
from rich.layout import Layout
from rich.padding import Padding
from rich.panel import Panel
from rich.progress_bar import ProgressBar
from rich.rule import Rule
from rich.segment import Segment, Segments
from rich.table import Table
from rich.text import Text
from solver import ParagraphReference

COMPOSITION_ROWS = 3


@dataclass(frozen=True)
class TextLayout:
    lines: list[Text]
    positions: list[tuple[int, int]]


def text_layout(target: str, typed: str, width: int) -> TextLayout:
    """Map source indices to cells, retaining source line breaks and tab stops."""
    lines, positions = [Text()], []
    column = 0
    for index, char in enumerate(target):
        wrong = index < len(typed) and typed[index] != char
        wrong |= len(typed) > len(target) and index == len(target) - 1
        style = (
            "bright_red"
            if wrong
            else "bright_black"
            if index < len(typed)
            else "default"
        )
        if char == "\n":
            positions.append((len(lines) - 1, column))
            lines.append(Text())
            column = 0
            continue
        visible = " " * (8 - column % 8) if char == "\t" else char
        size = cell_len(visible)
        if column and column + size > width:
            lines.append(Text())
            column = 0
        positions.append((len(lines) - 1, column))
        lines[-1].append(visible, style)
        column += size
    positions.append((len(lines) - 1, column))
    return TextLayout(lines, positions)


def token(key: NativeKey) -> str:
    names = {
        32: "␠",
        0xFF08: "⌫",
        0xFFFF: "Del",
        0xFF0D: "↵",
        0xFF09: "Tab",
        0xFF51: "←",
        0xFF53: "→",
    }
    value, states = key.keyval, key.states & CONTROL_MASK
    label = names.get(value, chr(value) if 32 <= value < 127 else f"Key({value:x})")
    return (
        ("Ctrl+" if states & 4 else "")
        + ("Alt+" if states & 8 else "")
        + ("Super+" if states & 64 else "")
        + label
    )


def sequence(keys: Iterable[NativeKey]) -> Text:
    return Text(" ".join(map(token, keys)), style="dim", overflow="fold")


def difference(value: int) -> str:
    if value > 0:
        return f"多用 {value} 键"
    if value < 0:
        return f"少用 {-value} 键"
    return "总键数相同"


def aligned_diff(diff: SequenceDiff, width: int) -> Group:
    """Wrap aligned token columns together, so inserts/deletes retain visible gaps."""
    rows: list[RenderableType] = []
    actual, reference = Text("实际 "), Text("参考 ")
    for block in diff.blocks:
        for a, b in zip_longest(block.actual, block.reference):
            label_a, label_b = token(a) if a else "·", token(b) if b else "·"
            size = max(cell_len(label_a), cell_len(label_b)) + 1
            if actual.cell_len > cell_len("实际 ") and actual.cell_len + size > width:
                rows.extend((actual, reference))
                actual, reference = Text("实际 "), Text("参考 ")
            style_a = "bold bright_red" if a and block.kind != "equal" else "dim"
            style_b = "bold bright_green" if b and block.kind != "equal" else "dim"
            actual.append(label_a, style_a).append(" " * (size - cell_len(label_a)))
            reference.append(label_b, style_b).append(" " * (size - cell_len(label_b)))
    if actual.cell_len > cell_len("实际 "):
        rows.extend((actual, reference))
    else:
        rows.append(Text("双方均无主键记录。", style="dim"))
    return Group(*rows)


def hand_label(hands: HandCounts) -> tuple[str, str]:
    left = f"{hands.left_ratio:.1%}" if hands.left_ratio is not None else "—"
    right = f"{hands.right_ratio:.1%}" if hands.right_ratio is not None else "—"
    return f"{hands.left} ({left})", f"{hands.right} ({right})"


def measured(value: float | None, unit: str) -> str:
    return "—" if value is None else f"{value:.2f} {unit}"


def source_diff(alignment: SourceAlignment, label: str, width: int) -> Group:
    """Three aligned rows per commit-span token, including unchanged source text."""
    console = Console(file=StringIO(), width=width)
    labels = ("原文", "实际", label)
    label_width = max(map(cell_len, labels)) + 2
    content_width = max(1, width - label_width)
    output: list[RenderableType] = []
    rows = [
        Text(value + " " * (label_width - cell_len(value)), style="dim")
        for value in labels
    ]
    occupied = 0

    def flush() -> None:
        nonlocal rows, occupied
        output.extend(rows)
        rows = [
            Text(value + " " * (label_width - cell_len(value)), style="dim")
            for value in labels
        ]
        occupied = 0

    for item in alignment.tokens:
        actual, expected = Text(), Text()
        for block in item.diff.blocks:
            for a, b in zip_longest(block.actual, block.reference):
                left, right = token(a) if a else "·", token(b) if b else "·"
                size = max(cell_len(left), cell_len(right))
                actual.append(
                    left, "bold bright_red" if a and block.kind != "equal" else "dim"
                ).append(" " * (size - cell_len(left)))
                expected.append(
                    right, "bold bright_green" if b and block.kind != "equal" else "dim"
                ).append(" " * (size - cell_len(right)))
        parts = [
            text.wrap(console, content_width, overflow="fold")
            for text in (Text(item.text), actual, expected)
        ]
        for index, triplet in enumerate(zip_longest(*parts, fillvalue=Text())):
            size = max(text.cell_len for text in triplet)
            if index:
                flush()
            if occupied and occupied + 2 + size > content_width:
                flush()
            if occupied:
                for row in rows:
                    row.append("  ")
                occupied += 2
            for row, text in zip(rows, triplet, strict=True):
                row.append_text(text).append(" " * (size - text.cell_len))
            occupied += size
    if occupied or alignment.tokens:
        flush()
    if alignment.corrections:
        output.append(
            Text("删改  ", style="dim").append_text(sequence(alignment.corrections))
        )
    return Group(*output)


def comparison_view(
    label: str, reference: ParagraphReference, pair: ReferenceComparison, width: int
) -> Group:
    status = {
        "optimal_in_scope": "范围内最优",
        "feasible_limit": "可行 · 未证最优",
        "unavailable": "不可用",
    }[reference["status"]]
    delta = difference(pair.extra) if pair.extra is not None else "无法比较"
    rows: list[RenderableType] = [
        Rule(f"实际 ↔ {label} · {delta} · {status}", align="left", style="cyan")
    ]
    if reference["status"] == "unavailable":
        failed = [
            part for part in reference["parts"] if part["status"] == "unavailable"
        ]
        rows.extend(
            Text(f"「{part['text']}」{part['note']}", style="bright_red")
            for part in failed
        )
        if not failed:
            rows.append(Text(reference["note"], style="bright_red"))
    elif pair.source is not None and pair.source.error is None:
        rows.append(source_diff(pair.source, label, width))
    elif pair.diff is not None:
        rows.extend(
            [
                Text(
                    (pair.source.error if pair.source else None) or "原文未对齐",
                    style="bright_red",
                ),
                aligned_diff(pair.diff, width),
            ]
        )
    else:
        rows.append(Text("参考： ").append_text(sequence(pair.keys)))
    return Group(*rows)


def results(
    native: NativeState,
    buffer: TypingBuffer,
    plan: PlanState,
    comparison: Comparison | None,
    *,
    width: int = 88,
) -> Group:
    metrics = comparison.metrics if comparison else typing_metrics(buffer, native)
    actual = metrics.counts
    sections: list[RenderableType] = [
        Text(
            f"{metrics.elapsed_seconds:.2f} 秒　{actual.total_keys} 键　{measured(metrics.keys_per_second, '键/秒')}　{measured(metrics.keys_per_minute, '键/分钟')}　{measured(metrics.characters_per_minute, '字/分钟')}",
            style="bold",
        ),
        Text(
            f"总键长 {measured(metrics.keys_per_character, '键/字')}　码长 {measured(metrics.codes_per_character, '码/字')}　错输 {buffer.error_insertions}　退格 {actual.ime_backspaces}　删除键 {actual.text_delete_keys}　删字 {buffer.deleted_characters}　修饰键 {actual.modifier_keys}",
            style="dim",
        ),
    ]
    if not metrics.capture_valid:
        sections.append(Text("采集无效：仅列已捕获按键", style="bright_red"))
    table = Table(box=None, padding=(0, 1), header_style="bold cyan", expand=False)
    titles = ["方案", "总键", "码键", "确认", "选重"]
    show_hands = width >= 88
    if show_hands:
        titles += ["左手", "右手", "空格", "未知"]
    for title in titles:
        table.add_column(
            title, justify="left" if title == "方案" else "right", no_wrap=True
        )
    entries = [("实际", actual, metrics.hands, True)]
    if comparison is not None and plan.references is not None:
        entries.extend(
            (
                label,
                pair.counts,
                pair.hands,
                plan.references[name]["status"] != "unavailable",
            )
            for label, name, pair in (
                ("最短", "shortest", comparison.shortest),
                ("少选重", "fewest_selections", comparison.fewest),
            )
        )
    for label, counts, hands, available in entries:
        values = [
            str(counts.total_keys),
            str(counts.code_keys),
            str(counts.confirmations),
            str(counts.second_selection + counts.third_selection),
        ]
        if show_hands:
            values += [*hand_label(hands), str(hands.space), str(hands.unknown)]
        table.add_row(
            label,
            *(values if available else ["—"] * len(values)),
            style="bold" if label == "实际" else "",
        )
    sections.append(table)
    if not show_hands:
        for label, _, hands, available in entries:
            if available:
                left, right = hand_label(hands)
                sections.append(
                    Text(
                        f"{label} 左 {left}  右 {right}  空格 {hands.space}  未知 {hands.unknown}",
                        style="dim",
                    )
                )
    if plan.status == "pending":
        sections.append(Text("正在计算参考…", style="dim"))
    elif plan.status == "error":
        sections.append(Text("参考计算失败：" + str(plan.error), style="bright_red"))
    elif comparison is not None and plan.references is not None:
        pairs = (
            ("最短", "shortest", comparison.shortest),
            ("少选重", "fewest_selections", comparison.fewest),
        )
        if width >= 120:
            grid = Table.grid(padding=(0, 3), expand=False)
            column_width = (width - 3) // 2
            grid.add_column(width=column_width)
            grid.add_column(width=column_width)
            grid.add_row(
                *(
                    comparison_view(label, plan.references[name], pair, column_width)
                    for label, name, pair in pairs
                )
            )
            sections.append(grid)
        else:
            sections.extend(
                comparison_view(label, plan.references[name], pair, width)
                for label, name, pair in pairs
            )
    return Group(*sections)


@dataclass(frozen=True)
class Frame:
    renderable: Layout
    size: os.terminal_size
    caret: tuple[int, int] | None = None  # 1-based, for the native IM popup.
    max_scroll: int = 0
    page_step: int = 1

    @property
    def rows(self) -> list[str]:
        """Plain render for snapshots and layout tests, using Rich's cell handling."""
        console = Console(
            file=StringIO(), width=self.size.columns, height=self.size.lines
        )
        return [
            "".join(segment.text for segment in line)
            for line in console.render_lines(self.renderable)
        ]


def screen(
    buffer: TypingBuffer,
    native: NativeState,
    plan: PlanState,
    size: os.terminal_size,
    comparison: Comparison | None = None,
    scroll: int = 0,
    *,
    source_path: str | None = None,
) -> Frame:
    columns, height = max(1, size.columns), max(1, size.lines)
    console = Console(file=StringIO(), width=columns, height=height)
    layout = Layout()
    if columns < 28 or height < 12:
        layout.update(Text("请放大终端窗口（至少 28 列、12 行）。"))
        return Frame(layout, size)
    complete = buffer.text == buffer.target
    title = Text(
        f"跟打已自动结束 · {len(buffer.target)} 字 · {buffer.elapsed_seconds:.1f} 秒"
        if complete
        else "键道跟打",
        style="bold",
    )
    if not complete:
        title.append(
            "  ·  输入即开始" if buffer.started_ns is None else "  ·  跟打中",
            style="dim" if buffer.started_ns is None else "cyan",
        )
    header_items: list[RenderableType] = [title]
    if source_path:
        header_items.append(Text("文件：" + source_path, style="dim", overflow="fold"))
    header_height = (
        len(
            console.render_lines(
                Group(*header_items),
                console.options.update(width=columns - 4, height=None),
            )
        )
        + 1
    )
    # Keep source metadata from consuming the practice area in narrow windows.
    header_height = min(header_height, height - COMPOSITION_ROWS - 6)
    footer_height = 3
    body_height = height - header_height - footer_height
    layout.split_column(
        Layout(
            Padding(Group(*header_items), (0, 2)), size=header_height, name="header"
        ),
        Layout(name="body"),
        Layout(size=footer_height, name="footer"),
    )
    caret = None
    max_scroll = 0
    panel_width = min(96, columns - 2)
    text_width = panel_width - 6
    if not complete:
        source = text_layout(buffer.target, buffer.text, text_width)
        cursor_row, cursor_column = source.positions[
            min(buffer.cursor, len(buffer.target))
        ]
        visible = min(len(source.lines), max(1, body_height - COMPOSITION_ROWS - 2))
        start = cursor_row // visible * visible
        shown = source.lines[start : start + visible]
        shown += [Text()] * (visible - len(shown))
        content = Group(
            *shown,
            *(Text() for _ in range(COMPOSITION_ROWS)),
        )
        panel_height = visible + COMPOSITION_ROWS + 2
        panel = Panel(
            content,
            width=panel_width,
            height=panel_height,
            padding=(0, 2),
            border_style="dim",
            title=Text("原文", style="dim"),
            title_align="left",
        )
        layout["body"].update(Align.center(panel, vertical="middle"))
        panel_top = header_height + max(0, (body_height - panel_height) // 2)
        panel_left = (columns - panel_width) // 2
        caret = (
            panel_top + 2 + visible,
            min(
                columns - 1,
                panel_left + 4 + min(cursor_column, text_width - 1),
            ),
        )
        status = Table.grid(expand=True, padding=(0, 2))
        status.add_column(ratio=1)
        status.add_column()
        status.add_row(
            ProgressBar(
                total=max(1, len(buffer.target)),
                completed=min(len(buffer.text), len(buffer.target)),
                style="dim",
                complete_style="cyan",
            ),
            Text(
                f"{min(len(buffer.text), len(buffer.target))}/{len(buffer.target)} 字　{buffer.elapsed_seconds:.1f}s",
                style="bright_red" if buffer.errors else "default",
            ),
        )
        warning = (
            "采集异常：" + native.errors[-1]
            if native.errors
            else "含粘贴，完成后不比较按键效率"
            if buffer.pasted_characters
            else ""
        )
        footer = Group(
            status,
            Text("F2 / Ctrl+D 退出 · 全对自动结束 · 完成后记日志", style="dim"),
            Text(warning, style="bright_red"),
        )
    else:
        text_width = columns - 4
        document = results(native, buffer, plan, comparison, width=text_width)
        lines = console.render_lines(
            document, console.options.update(width=text_width, height=None)
        )
        available = max(1, body_height)
        max_scroll = max(0, len(lines) - available)
        offset = min(max_scroll, max(0, scroll))
        segments = [
            segment
            for line in lines[offset : offset + available]
            for segment in (*line, Segment.line())
        ]
        layout["body"].update(Padding(Segments(segments), (0, 2)))
        notes = (
            Text("灰 相同 · ", style="dim")
            .append("红 实际差异", "bright_red")
            .append(" · ")
            .append("绿 参考差异", "bright_green")
            .append(
                " · · 缺键 | 按上屏片段对照；删改另列 | QWERTY估计，左右占比除以左+右，空格单列"
            )
        )
        if comparison and any(
            pair.diff
            and (
                pair.diff.coarse
                or (
                    pair.source and any(item.diff.coarse for item in pair.source.tokens)
                )
            )
            for pair in (comparison.shortest, comparison.fewest)
        ):
            notes.append(" | 长序列分块对齐")
        notes.no_wrap, notes.overflow = True, "ellipsis"
        footer = Group(
            Text(
                f"↑↓ PgUp/PgDn 滚动 · Home/End 首尾　{offset + 1}–{min(len(lines), offset + available)} / {len(lines)} 行"
                + (" · 已到末尾" if offset == max_scroll else ""),
                style="dim",
                no_wrap=True,
                overflow="ellipsis",
            ),
            notes,
            Text(
                "F2 / Ctrl+D 关闭结果 · 完成轮次留日志"
                + (
                    " · F3 重练额外操作最多的片段"
                    if comparison and comparison.retry_text
                    else ""
                ),
                style="dim",
                no_wrap=True,
                overflow="ellipsis",
            ),
        )
    layout["footer"].update(Padding(footer, (0, 2)))
    return Frame(layout, size, caret, max_scroll, max(1, body_height - 3))
