#!/usr/bin/env python3
"""Launch the Linux preview, or show its existing KDE tray application's window."""
import json
import os
from pathlib import Path
import subprocess
import sys


def bus_data(*arguments):
    result = subprocess.run(
        ["busctl", "--user", "--json=short", "--", *arguments],
        check=True, capture_output=True, text=True, timeout=3,
    )
    return json.loads(result.stdout)["data"]


def show_action(value):
    if isinstance(value, dict):
        return show_action(value.get("data"))
    if isinstance(value, list):
        if len(value) == 3 and isinstance(value[0], int) and isinstance(value[1], dict):
            label = value[1].get("label")
            if isinstance(label, dict):
                label = label.get("data")
            if label == "Open settings":
                return value[0]
        for child in value:
            action = show_action(child)
            if action is not None:
                return action
    return None


def show_running():
    try:
        items = bus_data("get-property", "org.kde.StatusNotifierWatcher",
                         "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                         "RegisteredStatusNotifierItems")
    except (OSError, subprocess.SubprocessError, ValueError, KeyError):
        return False
    for item in items:
        service, _, suffix = item.partition("/")
        path = "/" + suffix if suffix else "/StatusNotifierItem"
        matched = False
        try:
            if bus_data("get-property", service, path,
                        "org.kde.StatusNotifierItem", "Id") != "freesync":
                continue
            matched = True
            menu = bus_data("get-property", service, path,
                            "org.kde.StatusNotifierItem", "Menu")
            layout = bus_data("call", service, menu, "com.canonical.dbusmenu",
                              "GetLayout", "iias", "0", "-1", "1", "label")
            action = show_action(layout)
            if action is None:
                raise RuntimeError("FreeSync's Open settings action is unavailable.")
            subprocess.run(
                ["busctl", "--user", "call", service, menu,
                 "com.canonical.dbusmenu", "Event", "isvu",
                 str(action), "clicked", "i", "0", "0"],
                check=True, capture_output=True, text=True, timeout=3,
            )
            return True
        except (OSError, subprocess.SubprocessError, ValueError, KeyError):
            if matched:
                raise RuntimeError("Cannot open the running FreeSync window. Use its tray menu.")
            continue
    return False


def main():
    if show_running():
        return
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else (
        Path(__file__).resolve().parents[1] / "target/debug/freesync-desktop"
    )
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("Build FreeSync first: cargo build -p freesync-desktop")
    state = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local/state")) / "freesync"
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    log = state / "launcher.log"
    descriptor = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        process = subprocess.Popen(
            [str(binary)], stdin=subprocess.DEVNULL, stdout=descriptor,
            stderr=descriptor, start_new_session=True,
        )
    finally:
        os.close(descriptor)
    try:
        status = process.wait(timeout=1)
    except subprocess.TimeoutExpired:
        return
    if status:
        raise RuntimeError(f"FreeSync exited with status {status}. See {log}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as error:
        print(f"FreeSync: {error}", file=sys.stderr)
        sys.exit(1)
