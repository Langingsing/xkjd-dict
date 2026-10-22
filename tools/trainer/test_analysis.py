"""Metrics and both reference alignments use deterministic physical key data."""

import os
import unittest
from copy import deepcopy

from analysis import align_source, compare, typing_metrics
from keystrokes import hand_counts, sequence_diff
from model import NativeKey, NativeState, TypingBuffer
from planning import PlanState
from rich.console import Console
from rich.text import Text
from solver import References
from test_product import reference
from view import aligned_diff, results, screen, source_diff


def keys(text: str) -> tuple[NativeKey, ...]:
    return tuple(
        NativeKey(str(i), ord(char), 0, "", True) for i, char in enumerate(text)
    )


def fixture() -> tuple[TypingBuffer, NativeState, References]:
    buffer = TypingBuffer("甲乙")
    buffer.insert(buffer.target)
    buffer.started_ns, buffer.completed_ns = 0, 4_000_000_000
    native = NativeState()
    native.contexts = {"/test"}
    native.keys = {
        "a": NativeKey("a", ord("a"), 0, "", True, ""),
        "space": NativeKey("space", 32, 0, "a", True, "甲"),
        "b": NativeKey("b", ord("b"), 0, "", True, "乙"),
    }
    short, few = reference(), reference()
    few["keys"] = list(map(ord, "xyz"))
    few["trace"] = [
        {"key": ord(char), "handled": True, "commit": commit, "input_before": ""}
        for char, commit in zip("xyz", ("", "甲", "乙"), strict=True)
    ]
    return buffer, native, {"shortest": short, "fewest_selections": few}


class AnalysisTests(unittest.TestCase):
    def test_top_screen_key_shared_with_next_code_merges_source_tokens(self):
        buffer, native, refs = fixture()
        native.keys = {
            "first": NativeKey("first", ord("h"), 0, "", True),
            "top": NativeKey("top", ord("a"), 0, "h", True, "甲"),
            "select": NativeKey("select", ord("'"), 0, "a", True, "乙"),
        }
        aligned = align_source(buffer, native, refs["shortest"])
        self.assertEqual([item.text for item in aligned.tokens], ["甲乙"])
        self.assertEqual(len(aligned.tokens[0].actual), 3)

    def test_source_tokens_cover_unchanged_text_and_each_key_once(self):
        buffer, native, refs = fixture()
        for ref in (refs["shortest"], refs["fewest_selections"]):
            aligned = align_source(buffer, native, ref)
            self.assertIsNone(aligned.error)
            self.assertEqual(
                "".join(item.text for item in aligned.tokens), buffer.target
            )
            self.assertCountEqual(
                [key.key_id for item in aligned.tokens for key in item.actual]
                + [key.key_id for key in aligned.corrections],
                [key.key_id for key in native.keys.values()],
            )
        native.keys = {"a": NativeKey("a", 97, 0, "", True, "甲乙")}
        merged = align_source(buffer, native, refs["shortest"])
        self.assertEqual([item.text for item in merged.tokens], ["甲乙"])
        self.assertEqual(len(merged.tokens[0].actual), 1)
        self.assertEqual(len(merged.tokens[0].reference), 2)

    def test_deleted_commit_is_separate_and_reference_tail_is_not_hidden(self):
        buffer = TypingBuffer("甲乙")
        buffer.insert("甲错")
        buffer.delete()
        buffer.insert("乙")
        native = NativeState()
        values = (
            NativeKey("a", 97, 0, "", True, "甲"),
            NativeKey("wrong", 120, 0, "", True, "错"),
            NativeKey("del", 0xFF08, 0, "", False),
            NativeKey("b", 98, 0, "", True, "乙"),
        )
        native.keys = {key.key_id: key for key in values}
        ref = reference()
        alignment = align_source(buffer, native, ref)
        self.assertEqual("".join(item.text for item in alignment.tokens), "甲乙")
        self.assertEqual([key.key_id for key in alignment.corrections], ["wrong"])
        assigned = [key.key_id for item in alignment.tokens for key in item.actual] + [
            key.key_id for key in alignment.corrections
        ]
        self.assertCountEqual(assigned, native.keys)
        ref["trace"].append(
            {"key": 99, "commit": "", "handled": True, "input_before": ""}
        )
        self.assertIsNotNone(align_source(buffer, native, ref).error)

    def test_compact_source_diff_is_paired_on_wrap_and_has_no_prose(self):
        buffer, native, refs = fixture()
        alignment = align_source(buffer, native, refs["fewest_selections"])
        rows = [
            row
            for row in source_diff(alignment, "少选重", 14).renderables
            if isinstance(row, Text)
        ]
        self.assertEqual(len(rows) % 3, 0)
        styles = {span.style for row in rows for span in row.spans}
        self.assertTrue({"bold bright_red", "bold bright_green"} <= styles)
        for offset in range(0, len(rows), 3):
            self.assertEqual(
                [row.plain.split()[0] for row in rows[offset : offset + 3]],
                ["原文", "实际", "少选重"],
            )
            self.assertEqual(
                len({row.cell_len for row in rows[offset : offset + 3]}), 1
            )
            self.assertTrue(
                all(row.cell_len <= 14 for row in rows[offset : offset + 3])
            )
        self.assertEqual(
            "".join("".join(row.plain.split()[1:]) for row in rows[::3]), "甲乙"
        )

    def test_short_results_fit_one_screen_and_use_available_width(self):
        buffer, native, refs = fixture()
        comparison = compare(buffer, native, refs)
        for width in (100, 160):
            frame = screen(
                buffer,
                native,
                PlanState("ready", refs),
                os.terminal_size((width, 30)),
                comparison,
            )
            plain = "\n".join(frame.rows)
            self.assertEqual(frame.max_scroll, 0)
            self.assertNotIn("│", plain)
            self.assertIn("原文", plain)
            self.assertNotIn("QWERTY", "\n".join(frame.rows[:-3]))
            self.assertIn("红 实际差异", "\n".join(frame.rows[-3:]))

    def test_both_comparisons_are_independent_and_lossless(self):
        buffer, native, refs = fixture()
        result = compare(buffer, native, refs)
        self.assertEqual((result.shortest.extra, result.fewest.extra), (1, 0))
        self.assertNotEqual(result.shortest.diff, result.fewest.diff)
        for pair in (result.shortest, result.fewest):
            self.assertTrue(pair.valid)
            assert pair.diff is not None
            self.assertEqual(
                tuple(k for b in pair.diff.blocks for k in b.actual),
                result.metrics.keys,
            )
            self.assertEqual(
                tuple(k for b in pair.diff.blocks for k in b.reference), pair.keys
            )
        refs["shortest"]["status"] = "unavailable"
        refs["shortest"]["trace"] = []
        result = compare(buffer, native, refs)
        self.assertIsNone(result.shortest.diff)
        self.assertTrue(result.fewest.valid)

    def test_metrics_and_frozen_zero_time(self):
        buffer, native, _ = fixture()
        value = typing_metrics(buffer, native)
        self.assertEqual((value.elapsed_seconds, value.counts.total_keys), (4, 3))
        self.assertEqual(
            (value.keys_per_second, value.keys_per_minute, value.characters_per_minute),
            (0.75, 45, 30),
        )
        self.assertEqual(
            (value.keys_per_character, value.codes_per_character), (1.5, 1)
        )
        self.assertEqual(
            (value.hands.left, value.hands.right, value.hands.space), (2, 0, 1)
        )
        buffer.completed_ns = 0
        self.assertEqual(buffer.elapsed_seconds, 0)
        self.assertIsNone(typing_metrics(buffer, native).keys_per_second)

    def test_invalid_actual_does_not_fabricate_speed_or_diff(self):
        for reason in ("paste", "error"):
            buffer, native, refs = deepcopy(fixture())
            if reason == "paste":
                buffer.pasted_characters = 1
            else:
                native.errors.append("fixture")
            result = compare(buffer, native, refs)
            self.assertFalse(result.metrics.capture_valid)
            self.assertIsNone(result.metrics.keys_per_second)
            for pair in (result.shortest, result.fewest):
                self.assertIsNone(pair.extra)
                self.assertIsNone(pair.diff)

    def test_hand_denominator_excludes_space_modifiers_and_unknown(self):
        value = hand_counts(
            (
                *keys("bY'3 ;"),
                NativeKey("del", 0xFFFF, 0, "", False),
                NativeKey("unknown", 0x1008FF12, 0, "", False),
                NativeKey("shift", 0xFFE1, 0, "", False),
                NativeKey("quit", ord("d"), 4, "", False),
            )
        )
        self.assertEqual(
            (value.left, value.right, value.space, value.unknown), (2, 4, 1, 1)
        )
        assert value.left_ratio is not None and value.right_ratio is not None
        self.assertAlmostEqual(value.left_ratio, 1 / 3)
        self.assertAlmostEqual(value.right_ratio, 2 / 3)
        self.assertIsNone(hand_counts(keys(" ")).left_ratio)

    def test_alignment_empty_insert_delete_replace_and_bounded_long_input(self):
        for a, b in (
            ("", ""),
            ("abc", "abc"),
            ("ac", "abc"),
            ("abc", "ac"),
            ("ab", "xy"),
            ("", "abc"),
            ("abc", ""),
            ("ab" * 1000, "xy" * 1000),
        ):
            actual, reference_keys = keys(a), keys(b)
            diff = sequence_diff(actual, reference_keys)
            self.assertEqual(
                tuple(k for block in diff.blocks for k in block.actual), actual
            )
            self.assertEqual(
                tuple(k for block in diff.blocks for k in block.reference),
                reference_keys,
            )
            self.assertEqual(diff.coarse, len(a) > 1000)
        self.assertEqual(
            sequence_diff(
                (NativeKey("ctrl", 119, 4, "", False),), keys("w")
            ).actual_differences,
            1,
        )

    def test_ui_color_and_both_reports_survive_resize(self):
        diff = aligned_diff(sequence_diff(keys("abcde"), keys("a'3xe")), 14)
        styles = {
            span.style
            for row in diff.renderables
            if isinstance(row, Text)
            for span in row.spans
        }
        self.assertTrue({"bold bright_red", "bold bright_green"} <= styles)
        buffer, native, refs = fixture()
        comparison = compare(buffer, native, refs)
        plan = PlanState("ready", refs)
        console = Console(width=100)
        lines = console.render_lines(
            results(native, buffer, plan, comparison),
            console.options.update(height=None),
        )
        plain = "\n".join("".join(s.text for s in row) for row in lines)
        for label in (
            "实际 ↔ 最短",
            "实际 ↔ 少选重",
            "多用 1 键",
            "总键数相同",
            "0.75 键/秒",
            "45.00 键/分钟",
        ):
            self.assertIn(label, plain)
        for size in ((28, 12), (40, 18), (100, 30), (160, 50)):
            frame = screen(buffer, native, plan, os.terminal_size(size), comparison)
            self.assertEqual(len(frame.rows), size[1])
            self.assertIn("关闭结果", "\n".join(frame.rows[-3:]))


if __name__ == "__main__":
    unittest.main()
