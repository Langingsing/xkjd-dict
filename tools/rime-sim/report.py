"""Verify saved replay traces and produce local aggregate Markdown/JSON reports."""
import argparse
import hashlib
import json
from collections.abc import Iterable
from collections.abc import Set as AbstractSet
from pathlib import Path
from string import ascii_lowercase
from typing import TypedDict

from simulate import SELECT_KEYS, SELECTION_POLICY, schema_alphabet, source_codes


class KeyEvent(TypedDict):
    key: int
    handled: bool
    commit: str

class Counts(TypedDict):
    total: int
    code: int
    spaces: int
    selection: int
    second_selection: int
    third_selection: int
    auto_commits: int

def counts(events: Iterable[KeyEvent], alphabet: AbstractSet[str] = frozenset(ascii_lowercase)) -> Counts:
    # Schema punctuation is code input, but digits remain restricted to selector 3.
    code_keys = {ord(char) for char in alphabet if char.isascii() and char.isprintable()
        and not char.isdigit() and char not in SELECT_KEYS}
    result: Counts = {'total': 0, 'code': 0, 'spaces': 0, 'selection': 0,
        'second_selection': 0, 'third_selection': 0, 'auto_commits': 0}
    for event in events:
        key = event['key']
        result['total'] += 1
        if key in code_keys:
            result['code'] += 1
            result['auto_commits'] += bool(event['commit'])
        elif key == ord(' '):
            result['spaces'] += 1
        elif key == ord("'"):
            result['selection'] += 1
            result['second_selection'] += 1
        elif key == ord('3'):
            result['selection'] += 1
            result['third_selection'] += 1
        else:
            raise ValueError(f'unexpected key {key}')
        if not event['handled']:
            raise ValueError(f'unhandled key {key} in a successful replay')
    return result

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--samples', type=Path, required=True)
    parser.add_argument('--user', type=Path, required=True)
    parser.add_argument('--baseline-probe', type=Path, help='optional saved unmodified-schema probe JSON')
    args = parser.parse_args()
    samples = json.loads(args.samples.read_text())['samples']
    data = json.loads((args.results / 'result.json').read_text())
    if set(data) != {'xkjd6', 'xkjd6dz'}:
        raise ValueError('comparison requires exactly xkjd6 and xkjd6dz results')
    for mode, result in data.items():
        if result.get('selection_policy') != SELECTION_POLICY:
            raise ValueError(f'{mode}: unsupported selection policy; rerun simulation')
        if len(result['rows']) != len(samples):
            raise ValueError(f'{mode}: sample count mismatch')
        for index, (row, sample) in enumerate(zip(result['rows'], samples), 1):
            if (row['sample'], row['file'], row['line']) != (index, sample['file'], sample['line']):
                raise ValueError(f'{mode}: sample identity mismatch at {index}')
    completed = [index for index in range(len(samples)) if all(not data[mode]['rows'][index]['missing'] for mode in data)]
    summary = {'selected': len(samples), 'complete_common': len(completed),
        'han_characters': sum(sum(map(len, samples[i]['clauses'])) for i in completed), 'modes': {}}
    for mode, result in data.items():
        alphabet = schema_alphabet(args.user, mode)
        codes = source_codes(args.user, mode, (clause for sample in samples for clause in sample['clauses']), result['single'])
        for index, row in enumerate(result['rows']):
            for part in row['parts']:
                explicit = []
                for action in part['actions']:
                    if action['code'] not in codes.get(action['text'], set()):
                        raise ValueError('action is not a complete existing dictionary code')
                    rank = action['index']
                    if rank < -1 or rank >= len(SELECT_KEYS):
                        raise ValueError('action exceeds first three candidates')
                    expected = list(map(ord, action['code'] + ('' if rank == -1 else SELECT_KEYS[rank])))
                    if action['keys'] != expected:
                        raise ValueError('action uses unsupported selection keys')
                    explicit.extend(action['keys'])
                if explicit != part['explicit_keys']:
                    raise ValueError('explicit plan does not match its actions')
                if part['removed_spaces'] != len(explicit) - len(part['keys']):
                    raise ValueError('removed-space count mismatch')
                # The optimizer may only remove confirmation spaces, never code or selection keys.
                original = iter(explicit)
                for key in part['keys']:
                    for old in original:
                        if old == key:
                            break
                        if old != ord(' '):
                            raise ValueError('optimized plan altered a code or selection key')
                    else:
                        raise ValueError('optimized plan is not a subsequence of explicit plan')
                if any(key != ord(' ') for key in original):
                    raise ValueError('optimized plan dropped non-space suffix')
                if [event['key'] for event in part['trace']] != part['keys']:
                    raise ValueError('trace key mismatch')
                if ''.join(event['commit'] for event in part['trace']) != part['target'] or part['output'] != part['target']:
                    raise ValueError('trace output mismatch')
            if not row['missing'] and ''.join(part['output'] for part in row['parts']) != ''.join(samples[index]['clauses']):
                raise ValueError('sample output mismatch')
            row['counts'] = counts((event for part in row['parts'] for event in part['trace']), alphabet)
        common_rows = [result['rows'][i] for i in completed]
        summary['modes'][mode] = counts((event for row in common_rows for part in row['parts'] for event in part['trace']), alphabet)
        summary['modes'][mode]['removed_spaces'] = sum(part['removed_spaces'] for row in common_rows for part in row['parts'])
    manifest = [{'path': str(path.relative_to(args.user)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        for path in sorted(args.user.rglob('*')) if path.is_file() and 'build' not in path.relative_to(args.user).parts and not path.name.endswith('.userdb')]
    patch_file = args.user / 'xkjd6dz.custom.yaml'
    patch_text = patch_file.read_text().strip() if patch_file.exists() else None
    baseline = json.loads(args.baseline_probe.read_text()) if args.baseline_probe else None
    payload = {'summary': summary, 'isolated_files': manifest, 'samples_sha256': hashlib.sha256(args.samples.read_bytes()).hexdigest(),
        'dz_custom_yaml': patch_text, 'baseline_probe': baseline, 'selection_policy': SELECTION_POLICY}
    (args.results / 'summary.json').write_text(json.dumps(payload, ensure_ascii=False, indent=2))
    lines = ['# Rime 自动顶屏与候选选择模拟', '',
        f"原样本 {summary['selected']} 句，共同完整回放 {summary['complete_common']} 句、{summary['han_characters']} 个汉字。所有统计来自程序逐键回放，最终上屏必须逐字等于目标，LLM 不参与分词、取码、候选选择或核对", '',
        '使用本机 librime 和 librime-lua。中文、简体、emoji 关闭；其余选项依 schema 默认。dz 打开 danzi_mode 并限定单字分段。未复用桌面当前候选状态、学习历史或内存开关；标点分段，两边均不计标点', '',
        '**配置边界**：结果仅对应 --user 指定的隔离副本，不证明与桌面配置等价。源文件哈希、实际 dz 自定义配置和可选的原配置探针写入 summary.json', '',
        ('隔离副本的 xkjd6dz.custom.yaml：\n\n```yaml\n' + patch_text + '\n```') if patch_text is not None else '隔离副本没有 xkjd6dz.custom.yaml', '',
        ('原配置探针：\n\n```json\n' + json.dumps(baseline, ensure_ascii=False) + '\n```') if baseline is not None else '未附原配置探针，不对原配置的候选可达性作结论', '',
        '引擎错误日志保存在同目录 xkjd6.log 与 xkjd6dz.log；样本汉字段可达不表示所有组件均正常。逐键模拟不会替换错误组件的实现', '',
        "选重规则：只取首屏前三候选，首选用空格，次选用 '，三选用 3，不翻页，不使用 2、4–9 或其他数字键。每次确认或选重前必须打完该字词在源词库中已有的一条编码（可以是已有简码），不从未完成编码的补全候选中取词。自动上屏也只接受已有完整编码。程序用真实引擎验证每条路径", '',
        '搜索覆盖样本相关已有 schema 字母表 ASCII 编码。动态规划先找最少显式按键路径，随后贪心删空格，只有完整回放仍成功才保留，因此包含自动顶屏效果，但不保证全局最短。编码键触发上屏次数合计原生自动选择和 Lua 顶功，不能单凭该数区分两者。未做语境读音消歧', '',
        "|方案|编码键|空格|次选 ' |三选 3|合计|编码键触发上屏|省下空格|", '|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for mode, value in summary['modes'].items():
        lines.append(f"|{mode}|" + '|'.join(str(value[key]) for key in ('code','spaces','second_selection','third_selection','total','auto_commits','removed_spaces')) + '|')
    if len(completed) and 'xkjd6' in summary['modes'] and 'xkjd6dz' in summary['modes']:
        normal = summary['modes']['xkjd6']['total']; dz = summary['modes']['xkjd6dz']['total']
        lines += ['', f'该搜索范围内，词组方案相对隔离副本的 dz 单字方案少 {dz-normal} 键（{(dz-normal)/dz:.1%}）']
    lines += ['', '|样本|汉字|正常方案键数|dz 键数|', '|---|---:|---:|---:|']
    for index, sample in enumerate(samples):
        values = [str(data[mode]['rows'][index]['counts']['total']) if not data[mode]['rows'][index]['missing'] else '不可完整回放' for mode in ('xkjd6','xkjd6dz')]
        lines.append(f"|{index+1}|{sum(map(len,sample['clauses']))}|{'|'.join(values)}|")
    for index, sample in enumerate(samples):
        lines += ['', f'## 样本 {index+1}', '', f"来源：[{Path(sample['file']).name}](<{sample['file']}:{sample['line']}>)", '', sample['text']]
        for mode in ('xkjd6','xkjd6dz'):
            row = data[mode]['rows'][index]
            rendered = ' / '.join(''.join(chr(key) for key in part['keys']).replace(' ', '␠') for part in row['parts'])
            lines += ['', f'{mode}：`{rendered}`']
            if row['missing']:
                lines += ['', '缺少路径：' + '、'.join(row['missing'])]
    (args.results / 'comparison.md').write_text('\n'.join(lines) + '\n')
    print(json.dumps(summary, ensure_ascii=False))

if __name__ == '__main__':
    main()
