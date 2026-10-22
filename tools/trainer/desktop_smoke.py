"""Explicit fixed-input desktop verification; never used by the normal trainer command."""

import argparse
import json
import os
import subprocess
import sys
import time
from dataclasses import asdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "key-capture"))
from analysis import counts, reference_keys
from private_fcitx_trial import KEYS, stop
from recording import load_round
from solver import References


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--return-window", required=True, help="window selector to restore during cleanup")
    parser.add_argument(
        "--reference-log",
        type=Path,
        help="Replay the shortest path from a saved oracle.references.json; this is automation, not user measurement",
    )
    args = parser.parse_args()
    if not os.environ.get("CODEX_KDE_UI_GUARD_HELD"):
        parser.error("run under ui_guard.py run --indicator; see tools/key-capture/README.md")
    try:
        from kde_ui import (
            EV_KEY,
            KWIN_SERVICE,
            QDBUS,
            WINDOWS_IFACE,
            WINDOWS_RUNNER,
            UInputDevice,
            run,
            select_window,
        )
    except ModuleNotFoundError as error:
        parser.error(f"optional KDE automation dependency unavailable: {error}; set PYTHONPATH to its scripts directory")
    replay: References | None = (
        json.loads(args.reference_log.read_text()) if args.reference_log else None
    )
    target = (
        "".join(part["text"] for part in replay["shortest"]["parts"])
        if replay
        else "回看又视"
    )
    physical = {ord(char): (code, False) for char, code in KEYS.items()}
    physical.update(
        {0xFF0D: (28, False), 45: (12, False), 59: (39, False), 62: (52, True)}
    )
    if replay and any(key not in physical for key in replay["shortest"]["keys"]):
        parser.error("reference contains an unsupported physical key")
    session = args.session.resolve()
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, mode=0o700, exist_ok=False)
    previous = select_window(args.return_window)
    log = (evidence / "launcher.log").open("w")
    process = subprocess.Popen(
        [
            sys.executable,
            str(Path(__file__).with_name("launcher.py")),
            "--text",
            target,
            "--test-runtime-parent",
            str(session),
        ],
        stdout=log,
        stderr=log,
        text=True,
    )
    try:
        deadline = time.monotonic() + 150
        parent = session
        while True:
            candidates = list(parent.glob("xkjd-type-*/worker-ready"))
            if candidates:
                session = candidates[0].parent
                break
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("trainer did not become ready; inspect launcher.log")
            time.sleep(0.2)
        item = select_window("键道跟打")
        run(
            [
                QDBUS,
                KWIN_SERVICE,
                WINDOWS_RUNNER,
                f"{WINDOWS_IFACE}.Run",
                item["id"],
                "",
            ]
        )
        deadline = time.monotonic() + 20
        while not (session / "ime-ready").exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("private input context did not become ready")
            time.sleep(0.1)
        time.sleep(0.6)
        subprocess.run(
            ["spectacle", "-b", "-n", "-f", "-o", str(evidence / "before.png")],
            check=True,
        )
        print("Ready: inspect before.png and create evidence/continue", flush=True)
        deadline = time.monotonic() + 75
        while not (evidence / "continue").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("foreground verification timed out")
            time.sleep(0.1)
        with UInputDevice(
            b"Codex Trainer Smoke",
            keys=tuple(KEYS.values()) + (12, 39, 42, 52, 60, 61, 104, 109),
        ) as device:

            def press(code: int) -> None:
                device.emit(EV_KEY, code, 1)
                device.sync()
                time.sleep(0.04)
                device.emit(EV_KEY, code, 0)
                device.sync()
                time.sleep(0.12)

            if replay:
                for value in replay["shortest"]["keys"]:
                    code, shift = physical[value]
                    if shift:
                        device.emit(EV_KEY, 42, 1)
                        device.sync()
                    press(code)
                    if shift:
                        device.emit(EV_KEY, 42, 0)
                        device.sync()
            else:
                press(KEYS["h"])
                time.sleep(0.5)
                subprocess.run(
                    [
                        "spectacle",
                        "-b",
                        "-n",
                        "-f",
                        "-o",
                        str(evidence / "candidates.png"),
                    ],
                    check=True,
                )
                for char in "b\bbkf hb ":
                    press(KEYS[char])
                time.sleep(0.5)
                live = json.loads((session / "live.json").read_text())
                if not live["reference_hidden"]:
                    raise RuntimeError("reference was exposed while typing")
                if live["text"] != "回看会" or live["errors"] != 1:
                    raise RuntimeError(f"wrong/error stage not reached: {live}")
                subprocess.run(
                    ["spectacle", "-b", "-n", "-f", "-o", str(evidence / "error.png")],
                    check=True,
                )
                print(
                    "Error stage: inspect error.png, then create evidence/correct",
                    flush=True,
                )
                deadline = time.monotonic() + 75
                while not (evidence / "correct").exists():
                    if time.monotonic() > deadline:
                        raise RuntimeError("error display verification timed out")
                    time.sleep(0.1)
                for char in "\ba'ek3":
                    press(KEYS[char])
            time.sleep(0.5)
            deadline = time.monotonic() + 120
            while True:
                live = json.loads((session / "live.json").read_text())
                if live["completed"] and live["plan_status"] in ("ready", "error"):
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError("background reference timeout")
                time.sleep(0.1)
            if (
                not live["shortest_valid"]
                or not live["fewest_valid"]
                or live["extra_keys_shortest"] != (0 if replay else 7)
                or live["extra_keys_fewest"]
                != (
                    len(replay["shortest"]["keys"])
                    - len(replay["fewest_selections"]["keys"])
                    if replay
                    else 5
                )
                or (not replay and live["findings"] < 1)
                or not live["source_aligned_shortest"]
                or not live["source_aligned_fewest"]
            ):
                raise RuntimeError(f"comparison failed: {live}")
            expected = {
                "code_keys": 10,
                "confirmations": 2,
                "second_selection": 1,
                "third_selection": 1,
                "ime_backspaces": 1,
                "text_delete_keys": 1,
                "total_keys": 16,
            }
            if replay:
                expected = asdict(counts(reference_keys(replay["shortest"])))
                expected.pop("modifier_keys")  # Shift is a real physical key in replay.
            for key, value in expected.items():
                if live["counters"][key] != value:
                    raise RuntimeError(
                        f"{key}: expected {value}, got {live['counters'][key]}"
                    )
            if live["deleted_characters"] != (0 if replay else 1):
                raise RuntimeError("deletion count mismatch")
            log_directory = Path(
                json.loads((session / "session.json").read_text())["log_directory"]
            )
            recorded = load_round(log_directory / "round-0001.json")
            if (
                recorded.origin != "automation"
                or recorded.buffer.text != live["text"]
                or asdict(recorded.native.counters()) != live["counters"]
                or recorded.plan.references is None
            ):
                raise RuntimeError(
                    "completed-round recording differs from live measurement"
                )
            print(
                f"Verified actual capture log: {log_directory / 'round-0001.json'}",
                flush=True,
            )
            subprocess.run(
                ["spectacle", "-b", "-n", "-f", "-o", str(evidence / "completed.png")],
                check=True,
            )
            print(
                "Completed: inspect completed.png and create evidence/retry", flush=True
            )
            press(109)  # Physical PageDown must scroll results without adding keys.
            time.sleep(0.3)
            subprocess.run(
                ["spectacle", "-b", "-n", "-f", "-o", str(evidence / "diffs.png")],
                check=True,
            )
            live = json.loads((session / "live.json").read_text())
            if live["counters"]["total_keys"] != expected["total_keys"]:
                raise RuntimeError("result navigation changed frozen key count")
            deadline = time.monotonic() + 75
            while not (evidence / "retry").exists():
                if time.monotonic() > deadline:
                    raise RuntimeError("completion verification timed out")
                time.sleep(0.1)
            if not replay:
                press(61)
                time.sleep(0.5)
                live = json.loads((session / "live.json").read_text())
                if live["text"] or live["completed"] or not live["reference_hidden"]:
                    raise RuntimeError(f"retry did not reset practice: {live}")
                subprocess.run(
                    ["spectacle", "-b", "-n", "-f", "-o", str(evidence / "retry.png")],
                    check=True,
                )
            press(60)
        process.wait(timeout=10)
        if process.returncode:
            raise RuntimeError("trainer exited with failure")
        if list(parent.iterdir()):
            raise RuntimeError("temporary practice files survived exit")
        print("Desktop capture, editing, colors and exact counters passed", flush=True)
    finally:
        stop(process)
        log.close()
        run(
            [
                QDBUS,
                KWIN_SERVICE,
                WINDOWS_RUNNER,
                f"{WINDOWS_IFACE}.Run",
                previous["id"],
                "",
            ]
        )


if __name__ == "__main__":
    main()
