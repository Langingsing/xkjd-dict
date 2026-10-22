"""Private-bus regression: observed calls must not disconnect the monitor."""

import json
import select
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import dbus.bus
import dbus.lowlevel


class CaptureTests(unittest.TestCase):
    def test_observed_calls_do_not_produce_unknown_method_replies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "bus.conf"
            config.write_text(
                "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>"
                '<auth>EXTERNAL</auth><policy context="default">'
                '<allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/>'
                '<allow own="*"/></policy></busconfig>'
            )
            bus = subprocess.Popen(
                [
                    "dbus-daemon",
                    f"--config-file={config}",
                    "--nofork",
                    "--print-address=1",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            monitor = None
            sender = receiver = None
            try:
                assert bus.stdout is not None
                self.assertTrue(select.select([bus.stdout], [], [], 3)[0])
                address = bus.stdout.readline().strip()
                output = root / "events.jsonl"
                monitor = subprocess.Popen(
                    [
                        sys.executable,
                        str(Path(__file__).with_name("capture_bus.py")),
                        "--address",
                        address,
                        "--output",
                        str(output),
                    ],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                deadline = time.monotonic() + 4
                while not output.exists() or not output.stat().st_size:
                    if monitor.poll() is not None or time.monotonic() > deadline:
                        self.fail("observer startup failed")
                    time.sleep(0.02)
                sender, receiver = (
                    dbus.bus.BusConnection(address),
                    dbus.bus.BusConnection(address),
                )
                for value in (ord("h"), ord("b")):
                    message = dbus.lowlevel.MethodCallMessage(
                        receiver.get_unique_name(),
                        "/test",
                        "org.fcitx.Fcitx.InputContext1",
                        "ProcessKeyEvent",
                    )
                    message.append(value, 0, 0, False, 0, signature="uuubu")
                    sender.send_message(message)
                    time.sleep(0.15)
                deadline = time.monotonic() + 3
                while True:
                    events = [
                        json.loads(line) for line in output.read_text().splitlines()
                    ]
                    if sum(event["event"] == "key" for event in events) == 2:
                        break
                    if monitor.poll() is not None or time.monotonic() > deadline:
                        self.fail("monitor lost observed calls")
                    time.sleep(0.02)
                self.assertTrue(monitor.poll() is None)
                self.assertFalse(
                    any(event["event"] == "capture_error" for event in events)
                )
            finally:
                if sender is not None:
                    sender.close()
                if receiver is not None:
                    receiver.close()
                if monitor is not None:
                    monitor.terminate()
                    monitor.wait(timeout=4)
                bus.terminate()
                bus.wait(timeout=4)
                if bus.stdout is not None:
                    bus.stdout.close()
