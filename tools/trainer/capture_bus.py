"""Structured, session-local Fcitx observation. Never connects to the desktop session bus."""

import argparse
import importlib
import json
import os
import signal
import time
from pathlib import Path

import dbus
import dbus.bus
import dbus.lowlevel
from dbus.mainloop.glib import DBusGMainLoop

GLib = importlib.import_module("gi.repository.GLib")
GLibUnix = importlib.import_module("gi.repository.GLibUnix")

INTERFACE = "org.fcitx.Fcitx.InputContext1"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(args.address)
    loop = GLib.MainLoop()
    pending: dict[tuple[str, int], str] = {}
    with os.fdopen(fd, "w", buffering=1) as output:

        def write(event: str, **values) -> None:
            output.write(
                json.dumps(
                    {
                        "event": event,
                        "monotonic_ns": time.monotonic_ns(),
                        "epoch_ns": time.time_ns(),
                        **values,
                    },
                    ensure_ascii=False,
                )
                + "\n"
            )

        def message_filter(connection, message):
            kind = message.get_type()
            member = message.get_member()
            if (
                kind == dbus.lowlevel.MESSAGE_TYPE_METHOD_CALL
                and message.get_interface() == INTERFACE
            ):
                if member in ("ProcessKeyEvent", "ProcessKeyEventBatch"):
                    args = message.get_args_list()
                    key_id = f"{message.get_sender()}/{message.get_serial()}"
                    pending[(message.get_sender(), message.get_serial())] = key_id
                    write(
                        "key",
                        key_id=key_id,
                        context=message.get_path(),
                        keyval=int(args[0]),
                        keycode=int(args[1]),
                        states=int(args[2]),
                        release=bool(args[3]),
                        client_time=int(args[4]),
                        batch=member.endswith("Batch"),
                    )
            elif kind in (
                dbus.lowlevel.MESSAGE_TYPE_METHOD_RETURN,
                dbus.lowlevel.MESSAGE_TYPE_ERROR,
            ):
                key_id = pending.pop(
                    (message.get_destination(), message.get_reply_serial()), None
                )
                if key_id is not None:
                    args = message.get_args_list()
                    if kind == dbus.lowlevel.MESSAGE_TYPE_ERROR or not args:
                        write(
                            "key_error",
                            key_id=key_id,
                            error=str(message.get_error_name()),
                        )
                    else:
                        write("handled", key_id=key_id, handled=bool(args[-1]))
            elif (
                kind == dbus.lowlevel.MESSAGE_TYPE_SIGNAL
                and message.get_interface() == INTERFACE
            ):
                args = message.get_args_list()
                if member == "CommitString":
                    write("commit", context=message.get_path(), text=str(args[0]))
                elif member == "UpdateFormattedPreedit":
                    write(
                        "preedit",
                        context=message.get_path(),
                        text="".join(str(item[0]) for item in args[0]),
                        cursor=int(args[1]),
                    )
                elif member == "UpdateClientSideUI":
                    write(
                        "ui",
                        context=message.get_path(),
                        text="".join(str(item[0]) for item in args[0]),
                        candidates=[str(item[1]) for item in args[4]],
                        selected=int(args[5]),
                    )
            # Monitor copies must never enter dbus-python's object dispatcher:
            # replying UnknownMethod would cause the bus to disconnect us.
            return dbus.lowlevel.HANDLER_RESULT_HANDLED

        bus.add_message_filter(message_filter)
        bus.call_blocking(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.Monitoring",
            "BecomeMonitor",
            "asu",
            (
                [f"interface='{INTERFACE}'", "type='method_return'", "type='error'"],
                dbus.UInt32(0),
            ),
        )
        write("ready", source="private D-Bus Fcitx observer")

        def check_connection() -> bool:
            if not bus.get_is_connected():
                write("capture_error", error="私有 D-Bus 观察连接已断开")
                loop.quit()
                return False
            return True

        GLib.timeout_add_seconds(1, check_connection)
        for sig in (signal.SIGINT, signal.SIGTERM):
            GLibUnix.signal_add(
                GLib.PRIORITY_DEFAULT, sig, lambda: (loop.quit(), False)[1]
            )
        loop.run()
        write("stop", pending_replies=len(pending))
        bus.close()


if __name__ == "__main__":
    main()
