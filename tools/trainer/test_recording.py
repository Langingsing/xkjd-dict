"""Completed logs reproduce actual measurements, references and invalid captures."""

import json
import tempfile
import unittest
from dataclasses import asdict
from pathlib import Path
from unittest.mock import patch

from analysis import compare
from model import NativeState, TypingBuffer
from planning import PlanState
from recording import RoundRecorder, load_round
from test_analysis import fixture


class RecordingTests(unittest.TestCase):
    def test_roundtrip_preserves_actual_sequence_time_edits_and_both_references(self):
        buffer, native, refs = fixture()
        native.keys["a"].monotonic_ns = 123
        before = compare(buffer, native, refs)
        with tempfile.TemporaryDirectory() as directory:
            path = RoundRecorder(Path(directory), "/notes/test.md", "user").save(
                buffer, native, PlanState("ready", refs)
            )
            record = load_round(path)
            self.assertEqual(record.origin, "user")
            self.assertEqual(record.source_path, "/notes/test.md")
            assert record.plan.references is not None
            self.assertEqual(record.buffer.summary(), buffer.summary())
            self.assertEqual(
                list(record.native.keys.values()), list(native.keys.values())
            )
            self.assertEqual(
                asdict(compare(record.buffer, record.native, record.plan.references)),
                asdict(before),
            )
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_pending_plan_is_recorded_then_updated_without_overwriting_other_rounds(
        self,
    ):
        buffer, native, refs = fixture()
        with tempfile.TemporaryDirectory() as directory:
            recorder = RoundRecorder(Path(directory), None, "automation")
            first = recorder.save(buffer, native, PlanState())
            self.assertEqual(load_round(first).plan.status, "pending")
            stamp = json.loads(first.read_text())["completed_at"]
            self.assertEqual(
                recorder.save(buffer, native, PlanState("ready", refs), first), first
            )
            self.assertEqual(json.loads(first.read_text())["completed_at"], stamp)
            content = first.read_bytes()
            second = recorder.save(buffer, native, PlanState("error", error="timeout"))
            self.assertNotEqual(first, second)
            self.assertEqual(first.read_bytes(), content)
            self.assertEqual(load_round(second).plan.error, "timeout")
            self.assertEqual(load_round(second).origin, "automation")
            with self.assertRaises(FileExistsError):
                RoundRecorder(Path(directory), None, "user").save(
                    buffer, native, PlanState()
                )
            with self.assertRaisesRegex(ValueError, "只能更新当前轮次"):
                recorder.save(buffer, native, PlanState(), first)

    def test_unfinished_round_is_not_saved_and_replace_failure_preserves_old_log(self):
        with tempfile.TemporaryDirectory() as directory:
            recorder = RoundRecorder(Path(directory), None, "user")
            with self.assertRaisesRegex(ValueError, "completed"):
                recorder.save(TypingBuffer("甲乙"), NativeState(), PlanState())
            self.assertEqual(list(Path(directory).iterdir()), [])
            buffer, native, refs = fixture()
            path = recorder.save(buffer, native, PlanState())
            content = path.read_bytes()
            with (
                patch("recording.os.replace", side_effect=OSError("disk failure")),
                self.assertRaisesRegex(RuntimeError, "保存完成轮次失败"),
            ):
                recorder.save(buffer, native, PlanState("ready", refs), path)
            self.assertEqual(path.read_bytes(), content)
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_corrupt_records_are_rejected_with_path(self):
        buffer, native, refs = fixture()
        with tempfile.TemporaryDirectory() as directory:
            path = RoundRecorder(Path(directory), None, "user").save(
                buffer, native, PlanState("ready", refs)
            )
            original = json.loads(path.read_text())
            for field, value in (
                ("version", True),
                ("keys", [{"key_id": "bad"}]),
                ("completed_ns", -1),
                ("origin", "unknown"),
                ("edits", []),
            ):
                changed = dict(original)
                changed[field] = value
                path.write_text(json.dumps(changed))
                with self.assertRaisesRegex(ValueError, "round-0001.json"):
                    load_round(path)

    def test_deleted_wrong_text_and_capture_errors_remain_identifiable(self):
        buffer, native, refs = fixture()
        buffer = TypingBuffer(buffer.target)
        buffer.insert("甲错")
        buffer.delete()
        buffer.insert("乙")
        native.errors.append("missing association")
        with tempfile.TemporaryDirectory() as directory:
            path = RoundRecorder(Path(directory), None, "user").save(
                buffer, native, PlanState("ready", refs)
            )
            record = load_round(path)
            self.assertEqual(record.buffer.summary(), buffer.summary())
            self.assertEqual(record.native.errors, native.errors)
            assert record.plan.references is not None
            self.assertFalse(
                compare(
                    record.buffer, record.native, record.plan.references
                ).metrics.capture_valid
            )


if __name__ == "__main__":
    unittest.main()
