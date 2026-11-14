"""Launcher wrapper script generation for portable and extracted packages."""

from __future__ import annotations

import shlex
import stat
from pathlib import Path
from typing import Literal

InstallMode = Literal["portable", "extracted"]


def find_chrome_sandbox(root_dir: Path) -> Path | None:
    """Return the first chrome-sandbox binary inside *root_dir*, if present."""
    for candidate in root_dir.rglob("chrome-sandbox"):
        if candidate.is_file():
            return candidate
    return None


def has_suid_sandbox(sandbox_path: Path) -> bool:
    """Return True if *sandbox_path* is root-owned with the SUID bit set."""
    try:
        st = sandbox_path.stat()
    except OSError:
        return False
    return st.st_uid == 0 and bool(st.st_mode & stat.S_ISUID)


def _quote_arg(value: str) -> str:
    return shlex.quote(value)


def generate_wrapper_script(
    target_exec: Path,
    root_dir: Path | None,
    default_args: list[str],
    mode: InstallMode,
) -> str:
    """Generate an executable bash wrapper for launching an integrated app."""
    lines = ["#!/usr/bin/env bash", "set -euo pipefail", ""]

    extra_args = list(default_args)

    if mode == "extracted" and root_dir is not None:
        lib_dirs = [
            p
            for p in (
                target_exec.parent,
                root_dir / "usr" / "lib64",
                root_dir / "usr" / "lib",
                root_dir / "lib64",
                root_dir / "lib",
            )
            if p.is_dir()
        ]
        bin_dirs = [
            p
            for p in (
                target_exec.parent,
                root_dir / "usr" / "bin",
                root_dir / "usr" / "local" / "bin",
            )
            if p.is_dir()
        ]
        share_dir = root_dir / "usr" / "share"

        if lib_dirs:
            joined_libs = ":".join(str(p) for p in dict.fromkeys(lib_dirs))
            lines.append(
                f'export LD_LIBRARY_PATH="{joined_libs}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}"'
            )
        if bin_dirs:
            joined_bins = ":".join(str(p) for p in dict.fromkeys(bin_dirs))
            lines.append(f'export PATH="{joined_bins}:$PATH"')
        if share_dir.is_dir():
            lines.append(
                f'export XDG_DATA_DIRS="{share_dir}:${{XDG_DATA_DIRS:-/usr/local/share:/usr/share}}"'
            )

        sandbox = find_chrome_sandbox(root_dir)
        if sandbox is not None and not has_suid_sandbox(sandbox):
            lines.extend(
                [
                    'if [[ "${APPBOX_KEEP_SANDBOX:-0}" != "1" ]]; then',
                    '  set -- --no-sandbox "$@"',
                    "fi",
                ]
            )

    quoted_args = " ".join(_quote_arg(arg) for arg in extra_args)
    exec_cmd = f'exec "{target_exec}"'
    if quoted_args:
        exec_cmd += f" {quoted_args}"
    exec_cmd += ' "$@"'
    lines.extend(["", exec_cmd, ""])
    return "\n".join(lines)


def write_wrapper_script(dest_path: Path, script_content: str) -> Path:
    """Write *script_content* to *dest_path* and mark it executable."""
    dest_path.parent.mkdir(parents=True, exist_ok=True)
    dest_path.write_text(script_content, encoding="utf-8")
    dest_path.chmod(0o755)
    return dest_path
