"""Correlate actual Fcitx key_trace, CommitString signals and terminal samples."""
import argparse
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from datetime import datetime
from decimal import Decimal
import json
from pathlib import Path
import re
from zoneinfo import ZoneInfo

KEY_EVENT = re.compile(r'^D(?P<time>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d+) .*?KeyEvent: .*?origKey: Key\((?P<key>\S+) states=(?P<states>[^)]*)\) Release:(?P<release>[01]) keycode: (?P<code>\d+) program: (?P<program>.+)$')
COMMIT_HEADER = re.compile(r'^signal time=(?P<time>\d+\.\d+) .* path=(?P<path>\S+); .*member=CommitString$')
NS_PER_SECOND = 1_000_000_000


@dataclass(frozen=True)
class KeyEvent:
    epoch_ns: int
    key: str
    states: str
    release: bool
    code: int
    program: str


@dataclass(frozen=True)
class Commit:
    epoch_ns: int
    context_path: str
    text: str


def read_keys(path: Path, timezone: ZoneInfo) -> list[KeyEvent]:
    events = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if 'KeyEvent:' not in line:
            continue
        match = KEY_EVENT.fullmatch(line)
        if not match:
            raise ValueError(f'{path}:{number}: unrecognized key_trace event')
        timestamp = datetime.strptime(match['time'], '%Y-%m-%d %H:%M:%S.%f').replace(tzinfo=timezone)
        epoch_ns = int(timestamp.timestamp()) * NS_PER_SECOND + timestamp.microsecond * 1000
        events.append(KeyEvent(epoch_ns, match['key'], match['states'], match['release'] == '1', int(match['code']), match['program']))
    if not events:
        raise ValueError(f'{path}: no captured key events')
    if any(second.epoch_ns < first.epoch_ns for first, second in zip(events, events[1:])):
        raise ValueError(f'{path}: wall clock moved backwards; cannot correlate timestamps')
    return events


def read_commits(path: Path) -> list[Commit]:
    lines = path.read_text().splitlines()
    commits = []
    for number, line in enumerate(lines):
        match = COMMIT_HEADER.fullmatch(line)
        if not match:
            continue
        if number + 1 >= len(lines) or not lines[number + 1].strip().startswith('string '):
            raise ValueError(f'{path}:{number+1}: CommitString payload missing')
        text = json.loads(lines[number + 1].strip().removeprefix('string '))
        if not isinstance(text, str):
            raise ValueError('CommitString payload must be a string')
        commits.append(Commit(int(Decimal(match['time']) * NS_PER_SECOND), match['path'], text))
    if not commits:
        raise ValueError(f'{path}: no captured CommitString signals')
    return commits


def validate_key_pairs(events: Sequence[KeyEvent]) -> None:
    """The fixed experiment has no auto-repeat; check every sample's complete key cycles."""
    last_return = max((index for index, event in enumerate(events) if event.key == 'Return' and event.release), default=-1)
    if last_return < 0:
        raise ValueError('no complete sample Return key cycle')
    held: set[int] = set()
    for event in events[:last_return + 1]:
        if event.release:
            if event.code not in held:
                raise ValueError('key release without matching press; incomplete capture')
            held.remove(event.code)
        else:
            if event.code in held:
                raise ValueError('repeated key press without release; incomplete capture or unsupported auto-repeat')
            held.add(event.code)
    if held:
        raise ValueError('key presses without matching releases; incomplete capture')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--timezone', default='Asia/Shanghai', help='timezone of Fcitx textual log timestamps')
    args = parser.parse_args()
    folder = args.directory
    keys = read_keys(folder / 'fcitx.log', ZoneInfo(args.timezone))
    validate_key_pairs(keys)
    commits = read_commits(folder / 'commit-signals.log')
    tty = [event for line in (folder / 'tty.jsonl').read_text().splitlines()
        if (event := json.loads(line))['event'] == 'sample']
    if {event.program for event in keys} != {'konsole'} or len({event.context_path for event in commits}) != 1:
        raise ValueError('experiment must contain only one Konsole input context')
    rows = []
    current = []
    used_commits = []
    for event in keys:
        if event.release:
            continue
        if event.key != 'Return':
            current.append(event)
            continue
        if not current:
            raise ValueError('unexpected empty sample')
        matched = [commit for commit in commits if current[0].epoch_ns <= commit.epoch_ns <= event.epoch_ns]
        text = ''.join(commit.text for commit in matched)
        index = len(rows)
        if index >= len(tty) or text != tty[index]['display'] or tty[index]['hex'] != text.encode().hex():
            raise ValueError('CommitString does not match the actual terminal sample')
        used_commits.extend(matched)
        rows.append({'keys': [asdict(key) for key in current], 'commits': [asdict(commit) for commit in matched],
            'text': text, 'total_keys': len(current),
            'letter_keys': sum(len(key.key) == 1 and key.key.isascii() and key.key.isalpha() for key in current),
            'spaces': sum(key.key == 'space' for key in current),
            'backspaces': sum(key.key == 'BackSpace' for key in current),
            'second_selection': sum(key.key == 'apostrophe' for key in current),
            'third_selection': sum(key.key == '3' for key in current)})
        current = []
    if len(rows) != len(tty) or used_commits != commits:
        raise ValueError('unmatched terminal samples or commit signals')
    ledger_path = folder / 'injection-ledger.json'
    stimulus_verified = False
    if ledger_path.exists():
        sequences = json.loads(ledger_path.read_text())['sequences']
        aliases = {' ': 'space', '\b': 'BackSpace', "'": 'apostrophe', '\n': 'Return'}
        expected = [[aliases.get(key, key) for key in sequence] for sequence in sequences]
        observed = [[key['key'] for key in row['keys']] + ['Return'] for row in rows]
        if observed != expected:
            raise ValueError('captured keys do not match the fixed stimulus; incomplete or unexpected capture')
        stimulus_verified = True
    result = {'key_source':'Fcitx key_trace, independently observed before input-method processing',
        'commit_source':'D-Bus CommitString, cross-checked with terminal stdin',
        'timezone':args.timezone, 'samples':rows, 'stimulus_completeness_verified':stimulus_verified,
        'raw_events':len(keys), 'excluded_after_last_enter':[asdict(key) for key in current]}
    (folder / 'capture.json').write_text(json.dumps(result, ensure_ascii=False, indent=2))
    report = ['# 输入法侧按键捕获实验', '',
        '已在私有 D-Bus、独立 fcitx5/Rime 和受控 Konsole 窗口验证：输入法处理前按键与上屏文本均可捕获，不需要读取 /dev/input，也没有修改正在使用的输入法配置', '',
        '测试子进程的 QT_IM_MODULE 和 QT_IM_MODULES 均限定为 fcitx，绕过全局优先 Wayland 的入口。该结果仅证明受控 Qt/fcitx 会话，不能据此声称已记录现有 Wayland 桌面的任意应用', '',
        '原始键来自 fcitx 的 key_trace 日志，包含按下/释放、原始键、键码、修饰状态和日志接收时间；不是注入脚本的发送清单。上屏来自独立监听的 CommitString 信号，并与终端收到的 UTF-8 字节逐项相等。日志墙钟时间用于关联事件，不冒充硬件按下时刻', '',
        ('已核对按下/释放配对，并将独立捕获结果与固定注入清单逐键比较，完整性核验通过；注入清单未用于生成捕获数据' if stimulus_verified else '已核对按下/释放配对；未提供注入清单，未完成刺激完整性比较'), '',
        "|样本|实际捕获的按下序列（不含分隔回车）|字母键|空格|退格|次选 ' |三选 3|总键数|上屏|", '|---|---|---:|---:|---:|---:|---:|---:|---|']
    for index, row in enumerate(rows, 1):
        sequence = ' '.join(key['key'] for key in row['keys'])
        report.append(f"|{index}|`{sequence}`|{row['letter_keys']}|{row['spaces']}|{row['backspaces']}|{row['second_selection']}|{row['third_selection']}|{row['total_keys']}|{row['text']}|")
    report += ['', '样本由固定 uinput 自动化产生；尚未进行真人跟打效率测量。表中次选/三选列仅按捕获的 apostrophe/3 按键计数；没有命中不代表已验证这类操作。候选排序、修正动作与编码分段的正式归因，以及与最优路径的对比仍需后续实现', '',
        '原始记录：fcitx.log、commit-signals.log、tty.jsonl；机器核验结果：capture.json；输入刺激清单仅供对照：injection-ledger.json。普通桌面试验未成功接入私有 fcitx 的记录保留在 ../trial/，不计入本结论']
    (folder / 'capture.md').write_text('\n'.join(report) + '\n')
    print(json.dumps([{'text':row['text'],'keys':row['total_keys'],'backspaces':row['backspaces']} for row in rows], ensure_ascii=False))


if __name__ == '__main__':
    main()
