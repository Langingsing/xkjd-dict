"""Observe bytes delivered to a terminal, without intercepting or injecting keyboard events."""
import argparse
import datetime
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import termios
import time
import tty
from typing import TypedDict


class InputMethod(TypedDict):
    state: str | None
    engine: str | None


def input_method() -> InputMethod:
    def read(*args: str) -> str | None:
        try:
            result = subprocess.run(['fcitx5-remote', *args], capture_output=True, text=True, timeout=1)
            return result.stdout.strip() if result.returncode == 0 else None
        except (OSError, subprocess.TimeoutExpired):
            return None
    return {'state': read(), 'engine': read('-n')}


def show(text: str) -> None:
    # This function is also used with the terminal output processing disabled.
    sys.stdout.write(text.replace('\n', '\r\n'))
    sys.stdout.flush()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new JSONL file; refuses to overwrite')
    parser.add_argument('--seconds', type=float, default=300, help='maximum capture duration (default: 300 seconds)')
    args = parser.parse_args()
    if not sys.stdin.isatty():
        parser.error('stdin must be an interactive terminal')
    if not 0 < args.seconds <= 3600:
        parser.error('--seconds must be in (0, 3600]')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    fd = sys.stdin.fileno()
    original = termios.tcgetattr(fd)
    started = time.monotonic_ns()
    deadline = time.monotonic() + args.seconds
    with os.fdopen(descriptor, 'w', encoding='utf-8', buffering=1) as log:
        def record(event: str, **values: str | int | InputMethod) -> None:
            log.write(json.dumps({'event': event, 'elapsed_ns': time.monotonic_ns() - started, **values}, ensure_ascii=False) + '\n')

        record('start', utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
               terminal=os.ttyname(fd), input_method=input_method())
        show('CLI 输入探针：只观察终端 stdin，不监听其他窗口\n'
             '1. 请手动切英文，输入 hbkf 再按空格、回车\n'
             '2. 请手动切键道，正常输入“回看”，再按回车\n'
             f'也可自行测试次选和三选。Ctrl+D 或 Ctrl+C 结束，{args.seconds:g} 秒自动停止\n'
             f'记录：{args.output.resolve()}\n'
             '下面的 chunk 是终端读取批次，不等同于物理按键\n')
        pending = bytearray()
        reason = 'error'
        try:
            tty.setraw(fd)
            while time.monotonic() < deadline:
                ready, _, _ = select.select([fd], [], [], min(0.25, max(0, deadline - time.monotonic())))
                if not ready:
                    continue
                chunk = os.read(fd, 4096)
                if not chunk:
                    reason = 'eof'
                    break
                record('stdin_chunk', hex=chunk.hex())
                show(f'chunk: {chunk!r}\n')
                stop = False
                for byte in chunk:
                    if byte in (3, 4):
                        reason = 'ctrl_c' if byte == 3 else 'ctrl_d'
                        stop = True
                        break
                    if byte in (10, 13):
                        text = pending.decode('utf-8', errors='backslashreplace')
                        record('sample', hex=pending.hex(), display=text, input_method=input_method())
                        show(f'本轮终端字节解码：{text!r}\n')
                        pending.clear()
                    else:
                        pending.append(byte)
                if stop:
                    break
            else:
                reason = 'timeout'
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, original)
            record('stop', reason=reason, pending_hex=pending.hex())
        show('测试结束，终端设置已恢复\n')


if __name__ == '__main__':
    main()
