"""Product contracts: no answer leakage, distinct objectives and reusable builds."""

import argparse
import os
import pty
import re
import tempfile
import termios
import unittest
from pathlib import Path
from unittest.mock import patch

from analysis import (
    CAPTURE_SETTLE_NS,
    compare,
    completion_keys,
    completion_status,
    explain,
    groups,
)
from cache import prepare
from launcher import load_text, select_article
from model import Decoder, NativeKey, NativeState, TypingBuffer
from planning import PlanState
from rich.cells import cell_len
from rich.console import Console
from solver import ParagraphReference, References, State, shortest
from tui import consume_practice_actions, enable_raw_input
from view import results, screen, text_layout


def reference() -> ParagraphReference:
    return {
        "status": "optimal_in_scope",
        "keys": [97, 98],
        "parts": [],
        "trace": [
            {"key": 97, "handled": True, "commit": "甲", "input_before": ""},
            {"key": 98, "handled": True, "commit": "乙", "input_before": ""},
        ],
        "note": "fixture",
    }


class ProductTests(unittest.TestCase):
    def test_raw_input_preserves_terminal_output_newlines_for_rich(self):
        master, slave = pty.openpty()
        try:
            enable_raw_input(slave)
            attributes = termios.tcgetattr(slave)
            self.assertFalse(
                attributes[3] & (termios.ICANON | termios.ECHO | termios.ISIG)
            )
            os.write(slave, b"first\nsecond")
            self.assertEqual(os.read(master, 1024), b"first\r\nsecond")
            os.write(master, b"\x03")
            self.assertEqual(os.read(slave, 1024), b"\x03")
        finally:
            os.close(slave)
            os.close(master)

    def test_source_path_visible_during_practice_and_in_results(self):
        buffer, native = TypingBuffer("甲乙"), NativeState()
        path = "/docs/一层/二层/article.md"
        for complete in (False, True):
            with self.subTest(complete=complete):
                if complete:
                    buffer.insert(buffer.target)
                frame = screen(
                    buffer,
                    native,
                    PlanState(),
                    os.terminal_size((100, 30)),
                    source_path=path,
                )
                self.assertIn("文件：" + path, "\n".join(frame.rows))
                if frame.caret is not None:
                    self.assertLessEqual(frame.caret[0], 30)

    def test_unavailable_reference_shows_failed_part_and_reason(self):
        failed = reference()
        failed.update(
            status="unavailable",
            parts=[
                {
                    "text": "但有BUG，",
                    "keys": [],
                    "status": "unavailable",
                    "expanded": 6,
                    "seconds": 0.1,
                    "note": "规则范围内没有找到可回放路径",
                }
            ],
        )
        refs: References = {"shortest": failed, "fewest_selections": failed}
        buffer, native = TypingBuffer("但有BUG，"), NativeState()
        buffer.insert(buffer.target)
        frame = screen(
            buffer,
            native,
            PlanState("ready", refs),
            os.terminal_size((100, 80)),
            compare(buffer, native, refs),
        )
        shown = "\n".join(frame.rows)
        self.assertIn("但有BUG，", shown)
        self.assertIn("规则范围内没有找到可回放路径", shown)

    def test_directory_selects_from_all_depths_and_applies_paragraph(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            top = root / "top.md"
            top.write_text("顶层文章", encoding="utf-8")
            deep = root / "一层/二层/三层/article.TXT"
            deep.parent.mkdir(parents=True)
            deep.write_text("\ufeff第一段\n\n深层段落", encoding="utf-8")
            (root / "image.png").write_bytes(b"\x89PNG")
            (root / "folder.txt").mkdir()
            with patch("launcher.random.choice", return_value=deep) as choose:
                selected = select_article(root)
                text = load_text(
                    argparse.Namespace(file=selected, text=None, paragraph=2)
                )
            self.assertEqual(text, "深层段落")
            choose.assert_called_once()
            self.assertEqual(set(choose.call_args.args[0]), {top, deep})

    def test_directory_without_articles_reports_source_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "attachment.png").write_bytes(b"\x89PNG")
            with self.assertRaisesRegex(ValueError, str(root)):
                select_article(root)

    def test_exact_completion_locks_input_without_enter(self):
        buffer = TypingBuffer("甲乙")
        decoder = Decoder()
        self.assertEqual(
            consume_practice_actions(buffer, decoder.feed("甲乙\n错".encode())),
            "complete",
        )
        self.assertEqual(buffer.text, "甲乙")
        stopped = buffer.completed_ns
        consume_practice_actions(buffer, decoder.feed(b"\x7f"))
        self.assertEqual(buffer.text, "甲乙")
        self.assertEqual(buffer.completed_ns, stopped)

    def test_completion_excludes_later_physical_keys(self):
        buffer = TypingBuffer("甲")
        consume_practice_actions(buffer, Decoder().feed("甲\n".encode()))
        native = NativeState()
        native.keys = {
            "1": NativeKey("1", 97, 0, "", True, "甲"),
            "2": NativeKey("2", 0xFF0D, 0, "", False, ""),
            "3": NativeKey("3", 98, 0, "", None, ""),
        }
        self.assertEqual(completion_keys(buffer, native), [native.keys["1"]])
        assert buffer.completed_ns is not None
        self.assertEqual(
            completion_status(buffer, native, buffer.completed_ns), "ready"
        )
        # One indivisible commit exceeding the target cannot establish alignment.
        native.keys["1"].commit = "甲乙"
        self.assertIsNone(completion_keys(buffer, native))

    def test_completion_by_deleting_extra_character_keeps_final_delete(self):
        buffer = TypingBuffer("甲乙")
        buffer.insert("甲错乙")
        buffer.cursor = 2
        native = NativeState()
        native.keys = {
            "1": NativeKey("1", 97, 0, "", True, "甲错乙"),
            "2": NativeKey("2", 0xFF51, 0, "", False, ""),
        }
        consume_practice_actions(buffer, Decoder().feed(b"\x7f"))
        self.assertEqual(buffer.text, buffer.target)
        self.assertIsNone(completion_keys(buffer, native))
        native.keys["3"] = NativeKey("3", 0xFF08, 0, "", False, "")
        native.keys["4"] = NativeKey("4", 0xFF0D, 0, "", False, "")
        prefix = completion_keys(buffer, native)
        self.assertEqual(prefix, [native.keys[str(i)] for i in (1, 2, 3)])
        assert prefix is not None
        trimmed = NativeState()
        trimmed.keys = {key.key_id: key for key in prefix}
        self.assertEqual(trimmed.counters().text_delete_keys, 1)

    def test_completion_waits_for_late_commit_and_rejects_missing_capture(self):
        buffer = TypingBuffer("甲")
        buffer.insert("甲")
        native = NativeState()
        native.accept(
            {
                "event": "key",
                "key_id": "1",
                "context": "/test",
                "keyval": 97,
                "states": 0,
                "release": False,
            }
        )
        native.accept({"event": "handled", "key_id": "1", "handled": True})
        assert buffer.completed_ns is not None
        self.assertEqual(
            completion_status(buffer, native, buffer.completed_ns), "pending"
        )
        self.assertEqual(
            completion_status(buffer, native, buffer.completed_ns + CAPTURE_SETTLE_NS),
            "incomplete",
        )
        native.accept({"event": "commit", "context": "/test", "text": "甲"})
        self.assertEqual(
            completion_status(buffer, native, buffer.completed_ns), "ready"
        )

    def test_unhandled_key_keeps_commit_and_terminal_pass_through(self):
        commits, tail = groups([NativeKey("1", ord("."), 0, "a", False, "甲")])
        self.assertEqual([group.text for group in commits], ["甲."])
        self.assertFalse(tail)

    def test_missing_key_reply_cannot_leave_completed_practice_pending_forever(self):
        buffer = TypingBuffer("甲")
        buffer.insert("甲")
        native = NativeState()
        native.keys["1"] = NativeKey("1", 97, 0, "", None, "甲")
        assert buffer.completed_ns is not None
        self.assertEqual(
            completion_status(buffer, native, buffer.completed_ns + CAPTURE_SETTLE_NS),
            "incomplete",
        )

    def test_file_defaults_to_whole_text_and_paragraph_is_optional(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "article.txt"
            path.write_text("甲\n\n乙")
            self.assertEqual(
                load_text(argparse.Namespace(file=path, text=None, paragraph=None)),
                "甲\n\n乙",
            )
            self.assertEqual(
                load_text(argparse.Namespace(file=path, text=None, paragraph=2)), "乙"
            )

    def test_ready_references_are_hidden_until_completion(self):
        plan = PlanState(
            "ready", {"shortest": reference(), "fewest_selections": reference()}
        )
        buffer = TypingBuffer("甲乙")
        native = NativeState()
        rows = screen(buffer, native, plan, os.terminal_size((100, 30))).rows
        body = "\n".join(rows)
        self.assertNotIn("最短", body)
        self.assertNotIn("码键", body)
        self.assertNotIn("确认", body)
        self.assertNotIn("a b", body)
        self.assertIn("键道跟打", rows[0])
        self.assertIn(" " * 6, body)
        plain = [re.sub(r"\x1b\[[0-9;]*m", "", row) for row in rows]
        target = next(row for row in plain if "甲乙" in row)
        self.assertEqual(target.index("甲"), 5)
        buffer.insert("甲乙")
        self.assertIn(
            "正在计算",
            "\n".join(
                screen(buffer, native, PlanState(), os.terminal_size((100, 30))).rows
            ),
        )

    def test_source_stays_unchanged_and_caret_anchors_below_current_character(self):
        buffer = TypingBuffer("甲乙")
        buffer.insert("甲错")
        buffer.cursor = 1
        frame = screen(buffer, NativeState(), PlanState(), os.terminal_size((100, 30)))
        plain = [re.sub(r"\x1b\[[0-9;]*m", "", row) for row in frame.rows]
        source_row = next(i for i, row in enumerate(plain) if "甲乙" in row)
        self.assertNotIn("应为", "\n".join(plain))
        self.assertNotIn("当前", "\n".join(plain))
        self.assertEqual(plain[source_row].strip(" │"), "甲乙")
        self.assertNotIn("\x1b[4m", "".join(frame.rows))
        self.assertEqual(frame.caret, (source_row + 2, 8))
        lines = text_layout("甲\n乙", "错\n乙", 10).lines
        self.assertEqual("\n".join(row.plain for row in lines), "甲\n乙")

    def test_long_sequences_wrap_without_losing_keys(self):
        buffer, native = TypingBuffer("甲乙"), NativeState()
        buffer.insert(buffer.target)
        long_reference = reference()
        long_reference["trace"] = [
            {"key": ord(char), "handled": True, "commit": "", "input_before": ""}
            for char in "abc" * 100
        ]
        refs: References = {
            "shortest": long_reference,
            "fewest_selections": long_reference,
        }
        console = Console(width=40)
        lines = console.render_lines(
            results(
                native, buffer, PlanState("ready", refs), compare(buffer, native, refs)
            ),
            console.options.update(height=None),
        )
        plain = "\n".join("".join(segment.text for segment in row) for row in lines)
        self.assertEqual(
            [part for part in plain.split() if part in ("a", "b", "c")],
            list("abc" * 200),
        )
        self.assertTrue(all(cell_len(row) <= 40 for row in plain.splitlines()))

    def test_rich_layout_handles_resize_and_keeps_native_caret_in_practice(self):
        buffer = TypingBuffer("甲乙丙丁\n" * 40)
        buffer.insert("甲乙\n")
        for columns, height in ((28, 12), (40, 18), (100, 30), (160, 50)):
            with self.subTest(columns=columns, height=height):
                frame = screen(
                    buffer,
                    NativeState(),
                    PlanState(),
                    os.terminal_size((columns, height)),
                    source_path="/docs/文章.md",
                )
                self.assertEqual(len(frame.rows), height)
                self.assertTrue(all(cell_len(row) <= columns for row in frame.rows))
                assert frame.caret is not None
                row, column = frame.caret
                self.assertTrue(1 <= row <= height - 3)
                self.assertTrue(1 <= column <= columns)
                self.assertIn("退出", "\n".join(frame.rows[-3:]))

    def test_rich_result_scroll_reaches_end_without_moving_footer(self):
        buffer, native = TypingBuffer("甲乙"), NativeState()
        buffer.insert(buffer.target)
        long_reference = reference()
        long_reference["trace"] *= 80
        refs: References = {
            "shortest": long_reference,
            "fewest_selections": long_reference,
        }
        plan = PlanState("ready", refs)
        comparison = compare(buffer, native, refs)
        size = os.terminal_size((40, 18))
        first = screen(buffer, native, plan, size, comparison)
        last = screen(buffer, native, plan, size, comparison, scroll=10**6)
        self.assertGreater(last.max_scroll, 0)
        self.assertNotEqual(first.rows, last.rows)
        self.assertIn("参考：", "".join(last.rows))
        self.assertIn("灰 相同", "".join(last.rows[-3:]))
        self.assertIn("关闭结果", "\n".join(last.rows[-3:]))

    def test_normal_spacing_and_stable_rows_and_composition_area(self):
        buffer = TypingBuffer("甲乙\n丙丁\n戊己")
        native = NativeState()
        size = os.terminal_size((100, 30))

        def positions():
            frame = screen(buffer, native, PlanState(), size)
            plain = [re.sub(r"\x1b\[[0-9;]*m", "", row) for row in frame.rows]
            return {
                text: next(i for i, row in enumerate(plain) if text in row)
                for text in ("甲乙", "丙丁", "戊己")
            }, frame.caret

        before, caret_before = positions()
        self.assertEqual(before["丙丁"] - before["甲乙"], 1)
        self.assertEqual(before["戊己"] - before["丙丁"], 1)
        buffer.insert("甲乙\n")
        after, caret_after = positions()
        self.assertEqual(before, after)
        self.assertEqual(caret_before, caret_after)
        native.errors.append("fixture")
        with_warning, _ = positions()
        self.assertEqual(after, with_warning)

    def test_costs_are_attributed_once_after_correction(self):
        native = NativeState()

        def key(value: int, text: str = "", handled: bool = True):
            key_id = str(len(native.keys))
            native.accept(
                {
                    "event": "key",
                    "key_id": key_id,
                    "context": "/test",
                    "keyval": value,
                    "states": 0,
                    "release": False,
                }
            )
            native.accept({"event": "handled", "key_id": key_id, "handled": handled})
            if text:
                native.accept({"event": "commit", "context": "/test", "text": text})
            native.accept(
                {
                    "event": "key",
                    "key_id": key_id + "up",
                    "keyval": value,
                    "states": 0,
                    "release": True,
                }
            )

        buffer = TypingBuffer("甲乙")
        key(97, "甲")
        buffer.insert("甲")
        key(120, "错")
        buffer.insert("错")
        key(0xFF08, handled=False)
        buffer.delete()
        key(98, "乙")
        buffer.insert("乙")
        result = explain(buffer, native, reference())
        self.assertEqual(result.correction_keys, 1)
        self.assertEqual([(r.text, r.extra) for r in result.findings], [("乙", 1)])
        self.assertEqual(
            result.correction_keys + sum(r.extra for r in result.findings), 2
        )
        native.accept({"event": "commit", "context": "/test", "text": "外来上屏"})
        self.assertTrue(native.errors)

    def test_different_objectives_choose_different_paths(self):
        class Oracle:
            def run(self, keys, trace=False):
                table = {
                    "": ("", "", ()),
                    "a": ("", "a", ("乙", "甲")),
                    "a'": ("甲", "", ()),
                    "bb": ("", "bb", ("甲",)),
                    "bb ": ("甲", "", ()),
                }
                output, code, menu = table.get("".join(map(chr, keys)), ("#", "", ()))
                return State(output, code, code, menu, True, ())

        codes = {"甲": {"a", "bb"}}
        self.assertEqual(shortest("甲", codes, Oracle()).keys, tuple(map(ord, "a'")))
        self.assertEqual(
            shortest("甲", codes, Oracle(), objective="selections").keys,
            tuple(map(ord, "bb ")),
        )

    def test_cache_hit_content_invalidation_and_failed_build(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "sources"
            root.mkdir()
            cache = base / "cache"
            (root / "xkjd6.dict.yaml").write_text("one")
            (root / "lua").mkdir()
            (root / "lua/rules.lua").write_text("return 1")
            shared = base / "shared"
            shared.mkdir()
            calls = []

            def build(path: Path):
                calls.append(path)
                (path / "build").mkdir()
                (path / "build/xkjd6.schema.yaml").write_text("compiled")

            first = prepare(root, cache, shared=shared, build=build)
            self.assertFalse(first.reused)
            self.assertTrue(prepare(root, cache, shared=shared, build=build).reused)
            self.assertEqual(len(calls), 1)
            (root / "lua/rules.lua").write_text("return 2")
            second = prepare(root, cache, shared=shared, build=build)
            self.assertNotEqual(first.key, second.key)
            (root / "xkjd6.dict.yaml").write_text("broken")

            def fail(path: Path):
                raise RuntimeError("fixture compile error")

            with self.assertRaises(RuntimeError):
                prepare(root, cache, shared=shared, build=fail)
            self.assertTrue(first.path.exists())
            self.assertFalse(list(cache.glob(".build-*")))
