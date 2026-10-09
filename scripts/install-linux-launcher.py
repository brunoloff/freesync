#!/usr/bin/env python3
"""Install a per-user KDE/freedesktop menu entry for a built Linux preview."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys


def exec_quote(value):
    # Desktop Exec uses its own quoting, not shell quoting. Escape percent codes too.
    value = str(value).replace("%", "%%")
    value = "".join("\\" + c if c in '\\"`$' else c for c in value)
    return '"' + value.replace("\\", "\\\\") + '"'


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path,
                        default=root / "target/debug/freesync-desktop")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if sys.platform != "linux":
        parser.error("This launcher installer is for Linux.")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("Build the desktop binary before installing its launcher.")
    data = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
    applications = data / "applications"
    icons = data / "icons/hicolor/32x32/apps"
    applications.mkdir(parents=True, exist_ok=True)
    icons.mkdir(parents=True, exist_ok=True)
    icon = icons / "io.github.brunoloff.freesync.png"
    shutil.copyfile(root / "desktop/src-tauri/icons/icon.png", icon)
    entry = applications / "io.github.brunoloff.freesync.desktop"
    launch = root / "scripts/launch-desktop.py"
    entry.write_text(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=FreeSync\n"
        "Comment=Synchronize local folders with Google Drive\n"
        "Exec=" + " ".join(map(exec_quote, (sys.executable, launch, binary))) + "\n"
        "Icon=io.github.brunoloff.freesync\nTerminal=false\nStartupNotify=false\n"
        "StartupWMClass=Freesync-desktop\n"
        "Categories=Utility;FileTools;\nKeywords=sync;Google Drive;files;\n",
        encoding="utf-8",
    )
    entry.chmod(0o644)
    icon.chmod(0o644)
    validator = shutil.which("desktop-file-validate")
    if validator:
        subprocess.run([validator, str(entry)], check=True)
    cache = shutil.which("kbuildsycoca6") or shutil.which("kbuildsycoca5")
    if cache:
        subprocess.run([cache, "--noincremental"], check=True)
    print(f"Installed {entry}")


if __name__ == "__main__":
    main()
