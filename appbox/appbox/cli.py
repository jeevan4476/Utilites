"""Command-line interface for appbox."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from appbox import AppPaths, __version__, get_paths
from appbox.detect import UnsupportedPackageError
from appbox.integrator import install_package
from appbox.registry import (
    AppNotFoundError,
    list_manifests,
    load_manifest,
    remove_installed_app,
)

NAUTILUS_SCRIPT_NAME = "Install as App"


def _nautilus_script_content() -> str:
    repo_script = Path(__file__).resolve().parent.parent / "nautilus" / NAUTILUS_SCRIPT_NAME
    if repo_script.is_file():
        return repo_script.read_text(encoding="utf-8")
    return """#!/usr/bin/env bash
set -u

APPBOX_BIN="${HOME}/.local/bin/appbox"
if [[ ! -x "$APPBOX_BIN" ]]; then
  APPBOX_BIN="$(command -v appbox || true)"
fi

if [[ -z "$APPBOX_BIN" ]]; then
  zenity --error --title="AppBox" --text="Could not find 'appbox' in ~/.local/bin or PATH."
  exit 1
fi

mapfile -t SELECTED < <(printf '%s\\n' "${NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-}" | sed '/^$/d')
if [[ ${#SELECTED[@]} -eq 0 ]]; then
  zenity --error --title="AppBox" --text="Select one or more .AppImage or .rpm files first."
  exit 1
fi

for file in "${SELECTED[@]}"; do
  name="$(basename "$file")"
  lower="${name,,}"
  if [[ "$lower" != *.appimage && "$lower" != *.rpm ]]; then
    zenity --warning --title="AppBox" --text="Skipping unsupported file: $name"
    continue
  fi

  log_file="$(mktemp)"
  (
    echo "# Installing $name..."
    "$APPBOX_BIN" install "$file" >"$log_file" 2>&1
    rc=$?
    echo "$rc" > "${log_file}.rc"
  ) | zenity --progress --pulsate --auto-close --no-cancel --title="AppBox" --text="Installing $name..."

  rc="$(cat "${log_file}.rc" 2>/dev/null || echo 1)"
  output="$(cat "$log_file" 2>/dev/null || true)"
  rm -f "$log_file" "${log_file}.rc"

  if [[ "$rc" -ne 0 ]]; then
    zenity --error --title="AppBox Install Failed" --text="Failed to install $name:\\n\\n$output"
  else
    if grep -qi "warning:" <<<"$output"; then
      zenity --warning --title="AppBox Notice ($name)" --text="$output"
    fi
    if command -v notify-send >/dev/null 2>&1; then
      notify-send "AppBox" "Installed $name to your application menu."
    fi
  fi
done
"""


def install_nautilus_script(paths: AppPaths) -> Path:
    """Install the GNOME Files (Nautilus) right-click script."""
    paths.nautilus_scripts_dir.mkdir(parents=True, exist_ok=True)
    dest = paths.nautilus_scripts_dir / NAUTILUS_SCRIPT_NAME
    dest.write_text(_nautilus_script_content(), encoding="utf-8")
    dest.chmod(0o755)
    return dest


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="appbox",
        description="Install .AppImage and .rpm packages as user-space desktop apps.",
    )
    parser.add_argument("--version", action="version", version=f"appbox {__version__}")
    sub = parser.add_subparsers(dest="command", required=True)

    p_install = sub.add_parser("install", help="Install an .AppImage or .rpm package")
    p_install.add_argument("package", type=Path, help="Path to .AppImage or .rpm file")
    p_install.add_argument(
        "--extract",
        action="store_true",
        help="Extract AppImage contents instead of keeping a single binary in ~/Applications",
    )
    p_install.add_argument(
        "--move",
        action="store_true",
        help="Remove the original package file after a successful install",
    )
    p_install.add_argument(
        "--name",
        dest="app_name",
        help="Override the installed app ID / CLI command name",
    )

    p_list = sub.add_parser("list", help="List apps installed by appbox")
    p_list.add_argument("--json", action="store_true", help="Output JSON")

    p_info = sub.add_parser("info", help="Show details for an installed app")
    p_info.add_argument("app_id", help="Installed app ID")

    p_remove = sub.add_parser("remove", help="Uninstall an app installed by appbox")
    p_remove.add_argument("app_id", help="Installed app ID")

    sub.add_parser("setup-nautilus", help="Install GNOME Files right-click script")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    paths = get_paths()

    if args.command == "install":
        try:
            manifest = install_package(
                args.package.expanduser(),
                paths,
                app_id=args.app_name,
                extract=args.extract,
                move=args.move,
            )
        except (FileNotFoundError, UnsupportedPackageError, RuntimeError) as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1

        print(f"Installed {manifest.name} ({manifest.app_id}) [{manifest.package_type}/{manifest.install_mode}]")
        print(f"  Version : {manifest.version}")
        print(f"  CLI     : {manifest.bin_symlink}")
        if manifest.desktop_files:
            print(f"  Desktop : {manifest.desktop_files[0]}")
        if manifest.icon_files:
            print(f"  Icons   : {len(manifest.icon_files)} installed")
        for warning in manifest.warnings:
            print(f"Warning: {warning}", file=sys.stderr)
        return 0

    if args.command == "list":
        manifests = list_manifests(paths)
        if args.json:
            print(json.dumps([m.to_dict() for m in manifests], indent=2))
            return 0
        if not manifests:
            print("No apps installed via appbox.")
            return 0
        for m in manifests:
            print(f"{m.app_id:20s} {m.version:12s} {m.package_type:9s} {m.install_mode:10s} {m.name}")
        return 0

    if args.command == "info":
        manifest = load_manifest(args.app_id, paths)
        if manifest is None:
            print(f"Error: App '{args.app_id}' is not installed.", file=sys.stderr)
            return 1
        print(json.dumps(manifest.to_dict(), indent=2))
        return 0

    if args.command == "remove":
        try:
            removed = remove_installed_app(args.app_id, paths)
        except AppNotFoundError as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        print(f"Removed {removed.app_id} ({removed.name}).")
        return 0

    if args.command == "setup-nautilus":
        script_path = install_nautilus_script(paths)
        print(f"Installed Nautilus right-click script to {script_path}")
        return 0

    return 1
