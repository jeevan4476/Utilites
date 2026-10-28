"""CLI control and inspection utility for rclip."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from rclip_cli import __version__

BUS_NAME = "io.github.rclip"
OBJ_PATH = "/io/github/rclip"
IFACE_NAME = "io.github.rclip"


def _store_dir() -> Path:
    home = Path(os.environ.get("HOME", str(Path.home()))).expanduser()
    xdg = os.environ.get("XDG_DATA_HOME")
    data_home = Path(xdg).expanduser() if xdg else home / ".local" / "share"
    return data_home / "rclip"


def _history_path() -> Path:
    return _store_dir() / "history.json"


def _call_dbus(method: str) -> bool:
    """Invoke a D-Bus method on the running rclip GNOME Shell extension."""
    gdbus = shutil.which("gdbus")
    if not gdbus:
        return False
    proc = subprocess.run(
        [
            gdbus,
            "call",
            "--session",
            "--dest",
            BUS_NAME,
            "--object-path",
            OBJ_PATH,
            "--method",
            f"{IFACE_NAME}.{method}",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    return proc.returncode == 0


def _clear_on_disk() -> int:
    path = _history_path()
    if not path.is_file():
        return 0
    data = json.loads(path.read_text(encoding="utf-8"))
    items = data.get("items", [])
    kept = []
    removed = 0
    for item in items:
        if item.get("pinned"):
            kept.append(item)
        else:
            removed += 1
            img_path = item.get("imagePath")
            if img_path:
                Path(img_path).unlink(missing_ok=True)
    data["items"] = kept
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, indent=2), encoding="utf-8")
    tmp.chmod(0o600)
    tmp.replace(path)
    return removed


def _load_items() -> list[dict]:
    path = _history_path()
    if not path.is_file():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    return list(data.get("items", []))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="rclip",
        description="Windows Win+V style clipboard manager for GNOME Wayland.",
    )
    parser.add_argument("-v", "--version", action="version", version=f"rclip {__version__}")
    sub = parser.add_subparsers(dest="command")
    sub.add_parser("toggle", help="Toggle the clipboard popup")
    sub.add_parser("show", help="Show the clipboard popup")
    sub.add_parser("hide", help="Hide the clipboard popup")
    sub.add_parser("clear", help="Clear unpinned clipboard history")

    p_list = sub.add_parser("list", help="List stored clipboard items")
    p_list.add_argument("--json", action="store_true", help="Output raw JSON")

    sub.add_parser("status", help="Check GNOME extension and history status")

    args = parser.parse_args(argv)
    cmd = args.command or "toggle"

    if cmd in {"toggle", "show", "hide"}:
        method = cmd.capitalize()
        if _call_dbus(method):
            return 0
        print(
            "Error: rclip GNOME Shell extension is not active yet. "
            "If you just installed it, log out and back in once on Wayland, "
            "or run `gnome-extensions enable rclip@utilities.local`.",
            file=sys.stderr,
        )
        return 1

    if cmd == "clear":
        if _call_dbus("Clear"):
            print("Cleared unpinned clipboard history.")
            return 0
        removed = _clear_on_disk()
        print(f"Cleared {removed} unpinned item(s) on disk.")
        return 0

    if cmd == "list":
        items = _load_items()
        if args.json:
            print(json.dumps(items, indent=2))
            return 0
        if not items:
            print("Clipboard history is empty.")
            return 0
        for item in items:
            pin = "*" if item.get("pinned") else " "
            if item.get("type") == "image":
                desc = f"[image {item.get('width', 0)}x{item.get('height', 0)}] {item.get('imagePath', '')}"
            else:
                desc = " ".join(str(item.get("text", "")).splitlines())[:72]
            print(f"[{pin}] {item.get('id', '?'):20s} {desc}")
        return 0

    if cmd == "status":
        items = _load_items()
        print(f"History file : {_history_path()}")
        print(f"Stored items : {len(items)}")
        if shutil.which("gnome-extensions"):
            proc = subprocess.run(
                ["gnome-extensions", "info", "rclip@utilities.local"],
                capture_output=True,
                text=True,
                check=False,
            )
            if proc.returncode == 0:
                print(proc.stdout.strip())
            else:
                print("Extension    : rclip@utilities.local not installed")
        return 0

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
