import argparse
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from events import parse_native
from launcher import configure_practice_rime, load_text
from model import Decoder, NativeState, TypingBuffer
from rich.text import Span
from solver import Engine, State, shortest, solve_references
from view import text_layout


class TrainerTests(unittest.TestCase):
    def test_stuck_oracle_is_killed_and_log_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            program = root / "stuck-oracle"
            program.write_text("#!/usr/bin/python\nimport time\ntime.sleep(60)\n")
            program.chmod(0o700)
            engine = Engine(program, root, "xkjd6", False, root / "oracle.log")
            try:
                with (
                    patch("simulate.ORACLE_EXIT_SECONDS", 0.1),
                    self.assertRaisesRegex(RuntimeError, "was killed"),
                ):
                    engine.close()
                self.assertIsNotNone(engine.process.poll())
                self.assertTrue(engine.log.closed)
            finally:
                if engine.process.poll() is None:
                    engine.process.kill()
                    engine.process.wait(timeout=3)
                if engine.process.stdout is not None:
                    engine.process.stdout.close()

    def test_paragraph_loading_and_control_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "paragraphs.txt"
            path.write_text("\ufeff第一段\r\n\r\n回看又视\r\n下一行", encoding="utf-8")
            args = argparse.Namespace(text=None, file=path, paragraph=2)
            self.assertEqual(load_text(args), "回看又视\n下一行")
            args.paragraph = 3
            with self.assertRaises(ValueError):
                load_text(args)
        with self.assertRaises(ValueError):
            load_text(argparse.Namespace(text="字\x1b[31m", file=None, paragraph=1))

    def test_native_event_validation_and_disconnect(self):
        with self.assertRaises(TypeError):
            parse_native({"event": "key", "keyval": "h"})
        event = parse_native(
            {"event": "capture_error", "error": "disconnected", "monotonic_ns": 1}
        )
        state = NativeState()
        state.accept(event)
        self.assertEqual(state.errors, ["disconnected"])

    def test_decoder_split_utf8_and_controls_and_paste(self):
        decoder = Decoder()
        raw = "回".encode()
        self.assertEqual(decoder.feed(raw[:2]), [])
        self.assertEqual(decoder.feed(raw[2:])[0].text, "回")
        self.assertEqual(decoder.feed(b"\x1b["), [])
        self.assertEqual(decoder.feed(b"3~")[0].kind, "delete")
        self.assertEqual(decoder.feed(b"\x1b[5~")[0].kind, "page_up")
        events = decoder.feed(b"\x1b[200~abc\x1b[201~")
        self.assertEqual([event.kind for event in events], ["paste"] * 3)
        self.assertEqual(decoder.feed(b"\x1bOQ")[0].kind, "finish")

    def test_text_deletion_and_ime_backspace_are_distinct(self):
        buffer = TypingBuffer("回看")
        buffer.insert("回错")
        self.assertEqual(buffer.errors, 1)
        self.assertIn(
            Span(1, 2, "bright_red"),
            text_layout(buffer.target, buffer.text, 80).lines[0].spans,
        )
        self.assertIn(
            Span(0, 1, "bright_black"),
            text_layout(buffer.target, buffer.text, 80).lines[0].spans,
        )
        buffer.delete()
        buffer.insert("看")
        self.assertTrue(buffer.summary()["completed"])
        self.assertEqual(buffer.deleted_characters, 1)
        native = NativeState()
        for i, handled in enumerate((True, False)):
            native.preedit = "hb" if handled else ""
            native.accept(
                {
                    "event": "key",
                    "key_id": str(i),
                    "keyval": 0xFF08,
                    "states": 0,
                    "release": False,
                }
            )
            native.accept({"event": "handled", "key_id": str(i), "handled": handled})
        stats = native.counters()
        self.assertEqual((stats.ime_backspaces, stats.text_delete_keys), (1, 1))

    def test_confirmation_selection_and_releases(self):
        native = NativeState()
        native.preedit = "hb"
        for i, value in enumerate((32, 39, 51)):
            native.accept(
                {
                    "event": "key",
                    "key_id": str(i),
                    "keyval": value,
                    "states": 0,
                    "release": False,
                }
            )
            native.accept({"event": "handled", "key_id": str(i), "handled": True})
            native.accept(
                {
                    "event": "key",
                    "key_id": "release" + str(i),
                    "keyval": value,
                    "states": 0,
                    "release": True,
                }
            )
        stats = native.counters()
        self.assertEqual(
            (
                stats.total_keys,
                stats.confirmations,
                stats.second_selection,
                stats.third_selection,
            ),
            (3, 1, 1, 1),
        )


class Oracle:
    def run(self, keys, trace=False):
        text = "".join(map(chr, keys))
        table = {
            "": ("", "", ()),
            "a": ("", "a", ("甲",)),
            "aa": ("", "aa", ("甲",)),
            "a ": ("甲", "", ()),
            "ab": ("甲乙", "", ()),
            "cc": ("", "cc", ("甲乙",)),
            "cc ": ("甲乙", "", ()),
        }
        output, input_code, menu = table.get(text, ("#", "", ()))
        return State(output, input_code, input_code, menu, True, ())


class SolverTests(unittest.TestCase):
    def test_lowercase_english_can_commit_raw_despite_chinese_candidates(self):
        class MixedOracle:
            def run(self, keys, trace=False):
                output, pending = "", ""
                for key in keys:
                    if ord("a") <= key <= ord("z"):
                        if len(pending) == 2:
                            output += "错"
                            pending = ""
                        pending += chr(key)
                    elif key == 0xFF0D:
                        output += pending
                        pending = ""
                    else:
                        output += "错"
                        pending = ""
                return State(
                    output, pending, pending, ("汉",) if pending else (), True, ()
                )

        result = shortest("tag", {}, MixedOracle())
        self.assertEqual(result.status, "optimal_in_scope")
        self.assertEqual(len(result.keys), 5)
        self.assertIn(0xFF0D, result.keys)

    def test_disposable_rime_uses_only_xkjd6_and_starts_in_chinese(self):
        import yaml

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "build").mkdir()
            (root / "build/default.yaml").write_text(
                "schema_list: [{schema: other}]\nmenu: {page_size: 7}\n"
            )
            (root / "build/xkjd6.schema.yaml").write_text(
                "switches: [{name: full_shape, reset: 0}, {name: ascii_mode, reset: 1}]\n"
            )
            configure_practice_rime(root)
            default = yaml.safe_load((root / "build/default.yaml").read_text())
            schema = yaml.safe_load((root / "build/xkjd6.schema.yaml").read_text())
            self.assertEqual(default["schema_list"], [{"schema": "xkjd6"}])
            self.assertEqual(default["menu"]["page_size"], 7)
            self.assertEqual(schema["switches"][1]["reset"], 0)

    def test_reference_diagnostics_record_each_objective_and_final_result(self):
        for text in ("甲乙", "丙"):
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with (
                    patch(
                        "solver.source_codes", return_value={"甲": {"a"}, "乙": {"b"}}
                    ),
                    patch("solver.Engine") as engine,
                ):
                    engine.return_value.run.side_effect = Oracle().run
                    result = solve_references(
                        text, root, root / "engine", root / "oracle.log"
                    )
                entries = [
                    json.loads(line)
                    for line in (root / "oracle.jsonl").read_text().splitlines()
                ]
                self.assertEqual(
                    [entry["objective"] for entry in entries], ["keys", "selections"]
                )
                self.assertTrue(
                    all(entry["text"] == text and entry["note"] for entry in entries)
                )
                self.assertEqual(
                    json.loads((root / "oracle.references.json").read_text()), result
                )

    def test_raw_english_composition_can_top_up_or_commit_without_candidates(self):
        class EnglishOracle:
            def run(self, keys, trace=False):
                output, pending = "", ""
                for key in keys:
                    char = chr(key)
                    if char.isascii() and char.isalpha():
                        pending += char
                    elif char in (" ", ","):
                        output += pending + ("，" if char == "," else "")
                        pending = ""
                    else:
                        output = "#"
                return State(output, pending, pending, (), True, ())

        for text, expected in [("BUG，", "BUG,"), ("Bug", "Bug ")]:
            with self.subTest(text=text):
                result = shortest(text, {}, EnglishOracle())
                self.assertEqual(result.status, "optimal_in_scope")
                self.assertEqual(result.keys, tuple(map(ord, expected)))

    def test_wide_node_cannot_bypass_generated_work_limit(self):
        class WideOracle(Oracle):
            calls = 0

            def run(self, keys, trace=False):
                self.calls += 1
                return State("#" if tuple(keys) else "", "", "", (), True, ())

        oracle = WideOracle()
        result = shortest(
            "甲", {"甲": {f"a{i}" for i in range(1000)}}, oracle, max_states=1
        )
        self.assertLessEqual(oracle.calls, 65)
        self.assertEqual(result.status, "unavailable")
        self.assertIn("上限", result.note)

    def test_shorter_topup_path_beats_whole_word_confirmation(self):
        result = shortest(
            "甲乙", {"甲": {"a", "aa"}, "乙": {"b"}, "甲乙": {"cc"}}, Oracle()
        )
        self.assertEqual(result.status, "optimal_in_scope")
        self.assertEqual(result.keys, tuple(map(ord, "ab")))

    def test_search_limit_never_claims_optimal(self):
        result = shortest(
            "甲乙",
            {"甲": {"a", "aa"}, "乙": {"b"}, "甲乙": {"cc"}},
            Oracle(),
            max_states=1,
        )
        self.assertNotEqual(result.status, "optimal_in_scope")
        self.assertEqual(result.status, "feasible_limit")


if __name__ == "__main__":
    unittest.main()
