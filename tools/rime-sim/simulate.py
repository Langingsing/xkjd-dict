"""Deterministic candidate search and verified Rime key replay; no language model calls."""
from __future__ import annotations

import argparse
import json
import select
import subprocess
from collections import OrderedDict
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import TypedDict

import yaml

SELECT_KEYS = (' ', "'", '3')
ORACLE_CACHE_ENTRIES = 2048
ORACLE_REPLY_SECONDS = 30
ORACLE_EXIT_SECONDS = 5
SELECTION_POLICY = {'selectors': list(SELECT_KEYS), 'require_exact_code': True, 'paging': False}

class KeyEvent(TypedDict):
    key: int
    handled: bool
    commit: str
    input_before: str

class ActionData(TypedDict):
    text: str
    code: str
    keys: tuple[int, ...]
    index: int

class Replay(TypedDict):
    target: str
    actions: list[ActionData]
    explicit_keys: tuple[int, ...]
    keys: tuple[int, ...]
    output: str
    trace: tuple[KeyEvent, ...]
    removed_spaces: int

@dataclass(frozen=True)
class State:
    output: str
    preedit: str
    input: str
    menu: tuple[str, ...]
    last_page: bool
    trace: tuple[KeyEvent, ...]

@dataclass(frozen=True)
class Action:
    text: str
    code: str
    keys: tuple[int, ...]
    index: int

    def data(self) -> ActionData:
        return {'text': self.text, 'code': self.code, 'keys': self.keys, 'index': self.index}

class Engine:
    def __init__(self, executable: Path, user: Path, schema: str, single: bool, log: Path):
        self.schema = schema
        self.single = single
        self.log = log.open('w')
        self.process = subprocess.Popen([str(executable), str(user)], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.log, text=True)
        self.cache: OrderedDict[tuple[int, ...], State] = OrderedDict()
        self.calls = 0

    def run(self, keys: Iterable[int], trace: bool = False) -> State:
        keys = tuple(keys)
        if keys in self.cache and not trace:
            self.cache.move_to_end(keys)
            return self.cache[keys]
        request = {'schema': self.schema, 'single': self.single, 'keys': keys, 'trace': trace}
        assert self.process.stdin is not None and self.process.stdout is not None
        self.process.stdin.write(json.dumps(request) + '\n')
        self.process.stdin.flush()
        if not select.select([self.process.stdout], [], [], ORACLE_REPLY_SECONDS)[0]:
            raise TimeoutError(f'Rime oracle reply timed out; inspect {self.log.name}')
        response = self.process.stdout.readline()
        if not response:
            raise RuntimeError(f'engine stopped for {self.schema}; inspect {self.log.name}')
        data = json.loads(response)
        if 'error' in data:
            raise RuntimeError(data['error'])
        state = State(data['output'], data['preedit'], data['input'], tuple(data['menu']), data['last_page'], tuple(data['trace']))
        self.calls += 1
        if not trace:
            self.cache[keys] = state
            if len(self.cache) > ORACLE_CACHE_ENTRIES:
                self.cache.popitem(last=False)
        return state

    def close(self) -> None:
        assert self.process.stdin is not None
        timed_out = False
        try:
            try:
                self.process.stdin.close()
            except BrokenPipeError:
                pass  # The oracle may already have failed during a request.
            try:
                self.process.wait(timeout=ORACLE_EXIT_SECONDS)
            except subprocess.TimeoutExpired:
                timed_out = True
                self.process.kill()
                self.process.wait(timeout=ORACLE_EXIT_SECONDS)
        finally:
            if self.process.stdout is not None:
                self.process.stdout.close()
            self.log.close()
        if timed_out:
            raise RuntimeError(f'engine did not stop after EOF and was killed; inspect {self.log.name}')
        if self.process.returncode:
            raise RuntimeError(f'engine exited with status {self.process.returncode}')

def schema_alphabet(root: Path, schema: str) -> frozenset[str]:
    path = root / "build" / f"{schema}.schema.yaml"
    data: object = yaml.safe_load(path.read_text(encoding="utf-8"))
    speller = data.get("speller") if isinstance(data, dict) else None
    alphabet = speller.get("alphabet") if isinstance(speller, dict) else None
    if not isinstance(alphabet, str) or not alphabet:
        raise ValueError(f"missing or invalid speller/alphabet in {path}")
    return frozenset(alphabet)

def source_codes(root: Path, schema: str, clauses: Iterable[str], single: bool) -> dict[str, set[str]]:
    alphabet = schema_alphabet(root, schema)
    result = subprocess.run(['xkjd-dict', '--root', str(root), '--schema', schema, '--json', 'query', '--prefix', ''],
        text=True, capture_output=True, timeout=60, check=False)
    if result.returncode:
        raise RuntimeError(f'cannot index {schema} at {root}: {result.stderr}')
    clauses = tuple(clauses)
    codes: dict[str, set[str]] = {}
    for entry in json.loads(result.stdout):
        text, code = entry['text'], entry['code']
        if single and len(text) != 1:
            continue
        if code and code.isascii() and set(code) <= alphabet and any(text in clause for clause in clauses):
            codes.setdefault(text, set()).add(code)
    return codes

def selection_action(text: str, code: str, index: int, codes: Mapping[str, set[str]]) -> Action | None:
    if index < 0 or index >= len(SELECT_KEYS) or code not in codes.get(text, set()):
        return None
    return Action(text, code, tuple(map(ord, code + SELECT_KEYS[index])), index)

def discover(engine: Engine, codes: Mapping[str, set[str]]) -> dict[str, Action]:
    complete_codes = {code for values in codes.values() for code in values}
    actions: dict[str, Action] = {}
    for number, code in enumerate(sorted(complete_codes, key=lambda value: (len(value), value))):
        base = tuple(map(ord, code))
        state = engine.run(base)
        if state.output:
            if not state.preedit and code in codes.get(state.output, set()):
                action = Action(state.output, code, base, -1)
                if action.text not in actions or len(base) < len(actions[action.text].keys):
                    actions[action.text] = action
            continue
        for index, text in enumerate(state.menu[:len(SELECT_KEYS)]):
            action = selection_action(text, code, index, codes)
            if action is None:
                continue
            old = actions.get(text)
            score = (len(action.keys), index, code)
            if old is not None and score >= (len(old.keys), old.index, old.code):
                continue
            selected = engine.run(action.keys)
            if selected.output == text and not selected.preedit:
                actions[text] = action
        if number % 200 == 0:
            print(f'{engine.schema}: complete codes {number}/{len(complete_codes)}, targets {len(actions)}/{len(codes)}', flush=True)
    return actions

def segment(clause: str, actions: Mapping[str, Action]) -> list[Action] | None:
    plans: list[list[Action] | None] = [None] * (len(clause) + 1)
    plans[-1] = []
    for start in range(len(clause) - 1, -1, -1):
        best = None
        for end in range(start + 1, len(clause) + 1):
            action = actions.get(clause[start:end])
            tail = plans[end]
            if action is None or tail is None:
                continue
            plan = [action] + tail
            score = (sum(len(item.keys) for item in plan), len(plan))
            if best is None or score < best[0]:
                best = (score, plan)
        plans[start] = None if best is None else best[1]
    return plans[0]

def replay(engine: Engine, clause: str, plan: list[Action]) -> Replay:
    explicit = tuple(key for action in plan for key in action.keys)
    state = engine.run(explicit)
    if state.output != clause or state.preedit:
        raise RuntimeError(f'concatenated plan did not replay for {engine.schema}: {clause!r}')
    # Start with a valid full stream. Remove spaces only if the complete output remains exact.
    keys = explicit
    position = 0
    while position < len(keys):
        if keys[position] == ord(' '):
            trial = keys[:position] + keys[position + 1:]
            candidate = engine.run(trial)
            if candidate.output == clause and not candidate.preedit:
                keys = trial
                continue
        position += 1
    final = engine.run(keys, trace=True)
    if final.output != clause or final.preedit:
        raise RuntimeError('final replay failed')
    return {'target': clause, 'actions': [action.data() for action in plan],
        'explicit_keys': explicit, 'keys': keys, 'output': final.output,
        'trace': final.trace, 'removed_spaces': len(explicit) - len(keys)}

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', required=True, type=Path)
    parser.add_argument('--user', required=True, type=Path)
    parser.add_argument('--samples', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    samples = json.loads(args.samples.read_text())['samples']
    clauses = sorted({clause for row in samples for clause in row['clauses']})
    all_results = {}
    for schema, single in [('xkjd6', False), ('xkjd6dz', True)]:
        engine = Engine(args.engine.resolve(), args.user.resolve(), schema, single, args.output / f'{schema}.log')
        try:
            codes = source_codes(args.user, schema, clauses, single)
            actions = discover(engine, codes)
            results = []
            for index, sample in enumerate(samples, 1):
                parts = []
                missing = []
                for clause in sample['clauses']:
                    plan = segment(clause, actions)
                    if plan is None:
                        missing.append(clause)
                    else:
                        parts.append(replay(engine, clause, plan))
                results.append({'sample': index, 'file': sample['file'], 'line': sample['line'],
                    'parts': parts, 'missing': missing})
                print(f'{schema}: replay {index}/{len(samples)}, missing {len(missing)}', flush=True)
            all_results[schema] = {'single': single, 'selection_policy': SELECTION_POLICY,
                'oracle_calls': engine.calls, 'actions': {text: action.data() for text, action in actions.items()}, 'rows': results}
            (args.output / f'{schema}.json').write_text(json.dumps(all_results[schema], ensure_ascii=False, indent=2))
        finally:
            engine.close()
    (args.output / 'result.json').write_text(json.dumps(all_results, ensure_ascii=False, indent=2))

if __name__ == '__main__':
    main()
