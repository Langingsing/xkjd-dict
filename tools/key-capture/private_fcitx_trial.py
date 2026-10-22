"""Verify Fcitx key_trace against committed terminal text on an isolated session bus."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

TITLE = 'XKJD Isolated Fcitx Probe'
KEYS = {**dict(zip('qwertyuiop', range(16, 26))), **dict(zip('asdfghjkl', range(30, 39))),
    **dict(zip('zxcvbnm', range(44, 51))), ' ': 57, '\n': 28, '\b': 14, "'": 40, '3': 4}


def stop(process: subprocess.Popen) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True, help='prepared isolated config/data root')
    parser.add_argument('--output', type=Path, required=True, help='new output directory')
    parser.add_argument('--return-window', required=True, help='window selector to restore during cleanup')
    parser.add_argument('--selection-reference', type=Path, help='verified simulator result.json for deterministic second/third selection tests')
    args = parser.parse_args()
    args.work = args.work.resolve()
    args.output = args.output.resolve()
    if not os.environ.get('CODEX_KDE_UI_GUARD_HELD'):
        parser.error('run under ui_guard.py run --indicator')
    try:
        from kde_ui import UInputDevice, EV_KEY, select_window, run, QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, WINDOWS_IFACE
    except ModuleNotFoundError as error:
        parser.error(f'optional KDE automation dependency unavailable: {error}; set PYTHONPATH to its scripts directory')
    if not (args.work / 'config/fcitx5/profile').is_file() or not (args.work / 'data/fcitx5/rime/build/xkjd6.schema.yaml').is_file():
        parser.error('--work must contain an isolated Fcitx profile and compiled xkjd6 schema')
    cases = [('hbkf \n', '回看'), ('hb\bbkf \n', '回看')]
    if args.selection_reference:
        reference = json.loads(args.selection_reference.read_text())['xkjd6']['actions']
        for rank in (1, 2):
            eligible = [action for action in reference.values() if action['index'] == rank]
            if not eligible:
                parser.error(f'selection reference has no rank {rank+1} action')
            action = min(eligible, key=lambda item: (len(item['keys']), item['code'], item['text']))
            cases.append((''.join(map(chr, action['keys'])) + '\n', action['text']))
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    return_window = select_window(args.return_window)
    bus = subprocess.check_output(['dbus-daemon', '--session', '--fork', '--print-address=1', '--print-pid=1'], text=True).splitlines()
    address, bus_pid = bus[0], int(bus[1])
    with ExitStack() as cleanup:
        cleanup.callback(run, [QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, f'{WINDOWS_IFACE}.Run', return_window['id'], ''])
        cleanup.callback(os.kill, bus_pid, signal.SIGTERM)
        environment = dict(os.environ, DBUS_SESSION_BUS_ADDRESS=address,
            XDG_CONFIG_HOME=str(args.work / 'config'), XDG_DATA_HOME=str(args.work / 'data'),
            XDG_CACHE_HOME=str(args.work / 'cache'), QT_IM_MODULE='fcitx', QT_IM_MODULES='fcitx')
        # Isolate the input-method server's files, while preserving the user's
        # normal Konsole profile, font and colours in the client process.
        client_environment = dict(os.environ, DBUS_SESSION_BUS_ADDRESS=address,
            QT_IM_MODULE='fcitx', QT_IM_MODULES='fcitx')
        log = cleanup.enter_context((args.output / 'fcitx.log').open('w'))
        server = subprocess.Popen(['fcitx5', '-D', '-k', '--disable=all',
            '--enable=dbus,dbusfrontend,keyboard,rime', '--verbose=key_trace=5'],
            env=environment, stdout=log, stderr=log)
        cleanup.callback(stop, server)

        def remote(path: str, member: str, *values: str, check: bool = True) -> subprocess.CompletedProcess[str]:
            return subprocess.run([QDBUS, 'org.fcitx.Fcitx5', path, member, *values],
                env=environment, text=True, capture_output=True, check=check, timeout=5)

        deadline = time.monotonic() + 10
        while remote('/controller', 'org.fcitx.Fcitx.Controller1.State', check=False).returncode:
            if server.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('isolated fcitx failed to start; inspect fcitx.log')
            time.sleep(.2)
        commits = cleanup.enter_context((args.output / 'commit-signals.log').open('w'))
        monitor_error = cleanup.enter_context((args.output / 'monitor.stderr.log').open('w'))
        monitor = subprocess.Popen(['dbus-monitor', '--address', address,
            "type='signal',interface='org.fcitx.Fcitx.InputContext1',member='CommitString'"], stdout=commits, stderr=monitor_error)
        cleanup.callback(stop, monitor)
        terminal_log = cleanup.enter_context((args.output / 'konsole.log').open('w'))
        terminal = subprocess.Popen(['konsole', '--separate', '-p', f'LocalTabTitleFormat={TITLE}', '-e',
            sys.executable, str(Path(__file__).with_name('tty_probe.py')), '--output', str(args.output / 'tty.jsonl'), '--seconds', '120'],
            env=client_environment, stdout=terminal_log, stderr=terminal_log)
        cleanup.callback(stop, terminal)
        deadline = time.monotonic() + 15
        while not (args.output / 'tty.jsonl').exists():
            if terminal.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('isolated terminal failed to become ready; inspect konsole.log')
            time.sleep(.1)
        item = select_window(TITLE)
        run([QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, f'{WINDOWS_IFACE}.Run', item['id'], ''])
        time.sleep(.7)
        subprocess.run(['spectacle', '-b', '-n', '-f', '-o', str(args.output / 'before.png')], check=True)
        print('Ready: inspect before.png then create continue in output directory', flush=True)
        deadline = time.monotonic() + 75
        while not (args.output / 'continue').exists():
            if time.monotonic() > deadline:
                raise RuntimeError('foreground verification timed out')
            time.sleep(.1)
        remote('/controller', 'org.fcitx.Fcitx.Controller1.SetCurrentIM', 'rime')
        remote('/rime', 'org.fcitx.Fcitx.Rime1.SetSchema', 'xkjd6')
        remote('/rime', 'org.fcitx.Fcitx.Rime1.SetAsciiMode', 'false')
        time.sleep(.3)
        with UInputDevice(b'Codex Private Fcitx Probe', keys=tuple(KEYS.values()) + (29, 32)) as device:
            def press(code: int) -> None:
                device.emit(EV_KEY, code, 1)
                device.sync()
                time.sleep(.04)
                device.emit(EV_KEY, code, 0)
                device.sync()
                time.sleep(.1)

            for text, _ in cases:
                for char in text:
                    press(KEYS[char])
                time.sleep(.4)
            subprocess.run(['spectacle', '-b', '-n', '-f', '-o', str(args.output / 'after.png')], check=True)
            device.emit(EV_KEY, 29, 1)
            device.sync()
            press(32)
            device.emit(EV_KEY, 29, 0)
            device.sync()
        terminal.wait(timeout=5)
        (args.output / 'injection-ledger.json').write_text(json.dumps({'source':'fixed uinput test stimulus, not captured key events',
            'sequences':[text for text, _ in cases], 'private_bus':True, 'qt_im_module':'fcitx'}, ensure_ascii=False, indent=2))
        samples = [event for line in (args.output / 'tty.jsonl').read_text().splitlines()
            if (event := json.loads(line))['event'] == 'sample']
        if [event['display'] for event in samples] != [text for _, text in cases]:
            raise RuntimeError('input did not pass through isolated Rime as expected; inspect captured tty samples')


if __name__ == '__main__':
    main()
