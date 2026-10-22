"""Regression checks for complete-code selection and the owner's allowed keys."""
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from report import counts
from simulate import selection_action, source_codes


class SelectionPolicyTests(unittest.TestCase):
    def test_source_index_uses_compiled_schema_alphabet(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'build').mkdir()
            (root / 'build/xkjd6.schema.yaml').write_text(
                'speller:\n  alphabet: "abcdefghijklmnopqrstuvwxyz;"\n'
            )
            entries = [
                {'text': '、', 'code': ';d'},
                {'text': '！', 'code': ';t'},
                {'text': '甲', 'code': 'a'},
                {'text': '甲', 'code': 'a2'},
            ]
            completed = subprocess.CompletedProcess([], 0, json.dumps(entries), '')
            with patch('simulate.subprocess.run', return_value=completed):
                codes = source_codes(root, 'xkjd6', ['甲、！'], False)
            self.assertEqual(codes, {'、': {';d'}, '！': {';t'}, '甲': {'a'}})

    def test_incomplete_completion_is_rejected(self):
        codes = {'示例': {'ekjv', 'ek'}}
        self.assertIsNone(selection_action('示例', 'ekj', 1, codes))
        self.assertIsNone(selection_action('示例', 'e', 0, codes))
        short = selection_action('示例', 'ek', 1, codes)
        self.assertIsNotNone(short)
        assert short is not None
        self.assertEqual(short.keys, tuple(map(ord, "ek'")))

    def test_schema_punctuation_counts_as_code_without_allowing_numeric_codes(self):
        alphabet = frozenset("abcdefghijklmnopqrstuvwxyz;0123456789")
        result = counts([{'key': ord(key), 'handled': True, 'commit': ''}
            for key in ";d;t'3 "], alphabet)
        self.assertEqual((result['code'], result['second_selection'], result['third_selection']), (4, 1, 1))
        for key in '012456789':
            with self.subTest(key=key), self.assertRaises(ValueError):
                counts([{'key': ord(key), 'handled': True, 'commit': ''}], alphabet)

    def test_only_three_candidates_are_reachable(self):
        codes = {'示例': {'ekjv'}}
        third = selection_action('示例', 'ekjv', 2, codes)
        self.assertIsNotNone(third)
        assert third is not None
        self.assertEqual(third.keys, tuple(map(ord, 'ekjv3')))
        self.assertIsNone(selection_action('示例', 'ekjv', 3, codes))
        self.assertIsNone(selection_action('示例', 'ekjv', -1, codes))

    def test_disallowed_keys_cannot_enter_report(self):
        for key in '012456789=':
            with self.subTest(key=key), self.assertRaises(ValueError):
                counts([{'key': ord(key), 'handled': True, 'commit': '示例'}])
        result = counts([{'key': ord(key), 'handled': True, 'commit': ''} for key in "ek'3 "])
        self.assertEqual((result['total'], result['second_selection'], result['third_selection']), (5, 1, 1))


if __name__ == '__main__':
    unittest.main()
