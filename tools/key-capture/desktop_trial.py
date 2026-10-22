"""Run a bounded Konsole stdin experiment under the KDE UI guard; uses fixed test input."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import sys
import time

TITLE = 'XKJD CLI Key Probe'
KEY_CODES = {'h': 35, 'b': 48, 'k': 37, 'f': 33, ' ': 57, '\n': 28}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--return-window', required=True, help='window selector to activate during cleanup')
    args = parser.parse_args()
    if not os.environ.get('CODEX_KDE_UI_GUARD_HELD'):
        parser.error('run under ui_guard.py run --indicator; see tools/key-capture/README.md')
    try:
        from kde_ui import UInputDevice, EV_KEY, select_window, run, QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, WINDOWS_IFACE
    except ModuleNotFoundError as error:
        parser.error(f'optional KDE automation dependency unavailable: {error}; set PYTHONPATH to its scripts directory')
    args.output = args.output.resolve()
    args.evidence = args.evidence.resolve()
    if args.output.exists():
        parser.error('--output already exists; use a new filename')
    if args.evidence.exists() and any(args.evidence.iterdir()):
        parser.error('--evidence directory must be empty or new')
    args.evidence.mkdir(parents=True, exist_ok=True)
    prior_mode = subprocess.check_output(['fcitx5-remote'], text=True).strip()
    prior_engine = subprocess.check_output(['fcitx5-remote', '-n'], text=True).strip()
    return_window = select_window(args.return_window)
    probe = Path(__file__).with_name('tty_probe.py')
    terminal = subprocess.Popen(['konsole', '--separate', '-p', f'LocalTabTitleFormat={TITLE}',
        '-e', sys.executable, str(probe), '--output', str(args.output), '--seconds', '120'])
    trials = []

    def stop_terminal() -> None:
        if terminal.poll() is None:
            terminal.terminate()
            try:
                terminal.wait(timeout=5)
            except subprocess.TimeoutExpired:
                terminal.kill()
                terminal.wait(timeout=5)

    def return_focus() -> None:
        run([QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, f'{WINDOWS_IFACE}.Run', return_window['id'], ''])
        time.sleep(0.4)

    def save_trials() -> None:
        (args.evidence / 'trials.json').write_text(json.dumps({'prior_state': prior_mode,
            'prior_engine': prior_engine, 'trials': trials}, ensure_ascii=False, indent=2))

    # ExitStack attempts every callback even if an earlier cleanup action fails.
    with ExitStack() as cleanup:
        cleanup.callback(save_trials)
        cleanup.callback(subprocess.run, ['fcitx5-remote', '-o' if prior_mode == '2' else '-c'], check=True)
        if prior_engine:
            cleanup.callback(subprocess.run, ['fcitx5-remote', '-s', prior_engine], check=True)
        cleanup.callback(return_focus)
        cleanup.callback(stop_terminal)
        deadline = time.monotonic() + 15
        while not args.output.exists():
            if terminal.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Konsole probe failed to become ready')
            time.sleep(0.1)
        item = select_window(TITLE)
        run([QDBUS, KWIN_SERVICE, WINDOWS_RUNNER, f'{WINDOWS_IFACE}.Run', item['id'], ''])
        time.sleep(0.7)
        subprocess.run(['spectacle', '-b', '-n', '-f', '-o', str(args.evidence / 'before.png')], check=True)
        (args.evidence / 'ready.json').write_text(json.dumps(item, ensure_ascii=False))
        print('Ready: inspect before.png, then create continue file', flush=True)
        deadline = time.monotonic() + 75
        while not (args.evidence / 'continue').exists():
            if time.monotonic() > deadline:
                raise RuntimeError('No foreground verification received before timeout')
            time.sleep(0.1)
        with UInputDevice(b'Codex CLI Input Probe', keys=tuple(KEY_CODES.values()) + (29, 32, 42)) as device:
            def press(code: int) -> None:
                device.emit(EV_KEY, code, 1)
                device.sync()
                time.sleep(0.04)
                device.emit(EV_KEY, code, 0)
                device.sync()
                time.sleep(0.1)

            def type_trial(mode: str) -> None:
                before = subprocess.check_output(['fcitx5-remote'], text=True).strip()
                for char in 'hbkf \n':
                    press(KEY_CODES[char])
                time.sleep(0.4)
                trials.append({'requested_mode': mode, 'fcitx_state': before,
                    'injected_sequence': 'hbkf \n', 'source': 'uinput injection ledger, not a captured event stream'})

            subprocess.run(['fcitx5-remote', '-c'], check=True)
            time.sleep(0.3)
            type_trial('English/inactive')
            subprocess.run(['fcitx5-remote', '-s', 'rime'], check=True)
            subprocess.run(['fcitx5-remote', '-o'], check=True)
            time.sleep(0.5)
            type_trial('Rime/active')
            samples = [json.loads(line) for line in args.output.read_text().splitlines() if json.loads(line)['event'] == 'sample']
            if samples and samples[-1]['display'] == 'hbkf ':
                press(42)  # Rime's normal Shift ASCII toggle, only if the active engine still passed letters.
                type_trial('Rime/active after Shift toggle')
            subprocess.run(['spectacle', '-b', '-n', '-f', '-o', str(args.evidence / 'after.png')], check=True)
            device.emit(EV_KEY, 29, 1)
            device.sync()
            press(32)
            device.emit(EV_KEY, 29, 0)
            device.sync()
        terminal.wait(timeout=5)


if __name__ == '__main__':
    main()
