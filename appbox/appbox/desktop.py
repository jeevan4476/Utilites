"""Desktop entry parsing, rewriting, icon installation, and XDG cache refresh."""

from __future__ import annotations

import re
import shlex
import shutil
import struct
import subprocess
from dataclasses import dataclass
from pathlib import Path

from appbox import AppPaths

_FIELD_CODES = {"%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m"}
_PNG_SIG = b"\x89PNG\r\n\x1a\n"
_SIZE_DIR_RE = re.compile(r"(?:^|/)(\d+x\d+)(?:/|$)")


@dataclass(frozen=True)
class DesktopMetadata:
    """Parsed metadata from the primary [Desktop Entry] section."""

    name: str
    exec_cmd: str
    exec_args: list[str]
    field_codes: list[str]
    icon_name: str | None
    terminal: bool
    content: str


def _split_exec_line(raw_exec: str) -> tuple[str, list[str], list[str]]:
    """Split an Exec= value into (command, normal_args, desktop_field_codes)."""
    try:
        tokens = shlex.split(raw_exec)
    except ValueError:
        tokens = raw_exec.split()
    if not tokens:
        return "", [], []

    cmd = tokens[0]
    args: list[str] = []
    field_codes: list[str] = []
    for token in tokens[1:]:
        if token in _FIELD_CODES:
            field_codes.append(token)
        else:
            args.append(token)
    return cmd, args, field_codes


def parse_desktop_metadata(desktop_path: Path) -> DesktopMetadata:
    """Parse key metadata from [Desktop Entry] in *desktop_path*."""
    content = desktop_path.read_text(encoding="utf-8", errors="replace")
    in_main_section = False
    values: dict[str, str] = {}

    for line in content.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            in_main_section = stripped == "[Desktop Entry]"
            continue
        if in_main_section and "=" in line:
            key, val = line.split("=", 1)
            values.setdefault(key.strip(), val.strip())

    exec_cmd, exec_args, field_codes = _split_exec_line(values.get("Exec", ""))
    return DesktopMetadata(
        name=values.get("Name", desktop_path.stem),
        exec_cmd=exec_cmd,
        exec_args=exec_args,
        field_codes=field_codes,
        icon_name=values.get("Icon") or None,
        terminal=values.get("Terminal", "false").lower() == "true",
        content=content,
    )


def _format_exec_line(exec_target: Path, args: list[str], field_codes: list[str]) -> str:
    quoted_target = f'"{exec_target}"'
    parts = [quoted_target]
    for arg in args:
        if any(ch.isspace() or ch in '"\\$`' for ch in arg):
            escaped = arg.replace("\\", "\\\\").replace('"', '\\"')
            parts.append(f'"{escaped}"')
        else:
            parts.append(arg)
    parts.extend(field_codes)
    return " ".join(parts)


def rewrite_desktop_content(
    content: str,
    app_id: str,
    exec_target: Path,
    icon_ref: str,
    source_name: str,
) -> str:
    """Rewrite Exec=, TryExec=, Icon=, and AppBox tags in a .desktop file."""
    out_lines: list[str] = []
    current_section = ""
    saw_try_exec = False
    saw_icon = False
    saw_appbox_id = False
    saw_appbox_src = False

    def append_main_defaults() -> None:
        if current_section != "[Desktop Entry]":
            return
        if not saw_icon:
            out_lines.append(f"Icon={icon_ref}")
        if not saw_try_exec:
            out_lines.append(f"TryExec={exec_target}")
        if not saw_appbox_id:
            out_lines.append(f"X-AppBox-Id={app_id}")
        if not saw_appbox_src:
            out_lines.append(f"X-AppBox-Source={source_name}")

    for raw_line in content.splitlines():
        stripped = raw_line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            append_main_defaults()
            current_section = stripped
            out_lines.append(raw_line)
            continue

        if current_section == "[Desktop Entry]" and "=" in raw_line and not stripped.startswith("#"):
            key, val = raw_line.split("=", 1)
            key_clean = key.strip()
            if key_clean == "Exec":
                _, _, field_codes = _split_exec_line(val.strip())
                out_lines.append(f"Exec={_format_exec_line(exec_target, [], field_codes)}")
                continue
            if key_clean == "TryExec":
                saw_try_exec = True
                out_lines.append(f"TryExec={exec_target}")
                continue
            if key_clean == "Icon":
                saw_icon = True
                out_lines.append(f"Icon={icon_ref}")
                continue
            if key_clean == "X-AppBox-Id":
                saw_appbox_id = True
                out_lines.append(f"X-AppBox-Id={app_id}")
                continue
            if key_clean == "X-AppBox-Source":
                saw_appbox_src = True
                out_lines.append(f"X-AppBox-Source={source_name}")
                continue

        elif (
            current_section.startswith("[Desktop Action ")
            and "=" in raw_line
            and not stripped.startswith("#")
        ):
            key, val = raw_line.split("=", 1)
            key_clean = key.strip()
            if key_clean == "Exec":
                _, action_args, field_codes = _split_exec_line(val.strip())
                out_lines.append(
                    f"Exec={_format_exec_line(exec_target, action_args, field_codes)}"
                )
                continue
            if key_clean == "Icon":
                out_lines.append(f"Icon={icon_ref}")
                continue

        out_lines.append(raw_line)

    append_main_defaults()
    return "\n".join(out_lines).rstrip() + "\n"


def _detect_icon_format_and_size(icon_path: Path) -> tuple[str, str] | None:
    """Return (extension, hicolor_size_dir) for PNG or SVG icons, else None."""
    try:
        header = icon_path.read_bytes()[:64]
    except OSError:
        return None

    if header.startswith(_PNG_SIG) and len(header) >= 24 and header[12:16] == b"IHDR":
        width, height = struct.unpack(">II", header[16:24])
        if width > 0 and height > 0:
            return "png", f"{width}x{height}"
        match = _SIZE_DIR_RE.search(icon_path.as_posix())
        return "png", match.group(1) if match else "256x256"

    suffix = icon_path.suffix.lower()
    if suffix == ".svg" or b"<svg" in header.lower():
        return "svg", "scalable"

    return None


def find_icon_candidates(root_dir: Path, icon_hint: str | None = None) -> list[Path]:
    """Find candidate PNG/SVG icon files inside an extracted package tree."""
    candidates: list[Path] = []
    seen: set[Path] = set()
    hint_stem = Path(icon_hint).stem.lower() if icon_hint else None

    for path in root_dir.rglob("*"):
        if path in seen or path.is_dir():
            continue
        # Follow valid symlinks but skip broken ones.
        try:
            resolved = path.resolve(strict=True)
        except OSError:
            continue
        if not resolved.is_file():
            continue

        if path.name == ".DirIcon" or path.suffix.lower() in {".png", ".svg"}:
            seen.add(path)
            candidates.append(path)

    def sort_key(p: Path) -> tuple[int, int, str]:
        stem_match = 0 if (hint_stem and p.stem.lower() == hint_stem) else 1
        in_icons_dir = 0 if ("icons" in p.parts or p.name == ".DirIcon") else 1
        return (stem_match, in_icons_dir, p.as_posix())

    candidates.sort(key=sort_key)
    return candidates


def install_best_icons(
    candidates: list[Path],
    app_id: str,
    hicolor_dir: Path,
) -> list[Path]:
    """Install unique PNG/SVG icon sizes for *app_id* into *hicolor_dir*."""
    installed: list[Path] = []
    installed_slots: set[tuple[str, str]] = set()

    for candidate in candidates:
        detected = _detect_icon_format_and_size(candidate)
        if not detected:
            continue
        ext, size_dir = detected
        slot = (size_dir, ext)
        if slot in installed_slots:
            continue

        dest = hicolor_dir / size_dir / "apps" / f"{app_id}.{ext}"
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(candidate.resolve(), dest)
        dest.chmod(0o644)
        installed_slots.add(slot)
        installed.append(dest)

    return installed


def pick_primary_desktop(desktop_files: list[Path]) -> Path | None:
    """Select the most likely primary .desktop file from extracted candidates."""
    if not desktop_files:
        return None
    # Prefer top-level or usr/share/applications/*.desktop entries with Type=Application.
    scored: list[tuple[int, int, str, Path]] = []
    for path in desktop_files:
        text = path.read_text(encoding="utf-8", errors="replace")
        is_app = 0 if "Type=Application" in text else 1
        no_display = 1 if "NoDisplay=true" in text else 0
        depth = len(path.parts)
        scored.append((no_display, is_app, f"{depth:03d}:{path.name}", path))
    scored.sort(key=lambda item: (item[0], item[1], item[2]))
    return scored[0][3]


def refresh_desktop_caches(paths: AppPaths) -> None:
    """Refresh GNOME/XDG desktop and icon caches (best-effort)."""
    if paths.desktop_dir.exists() and shutil.which("update-desktop-database"):
        subprocess.run(
            ["update-desktop-database", str(paths.desktop_dir)],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    if paths.hicolor_dir.exists() and shutil.which("gtk-update-icon-cache"):
        subprocess.run(
            ["gtk-update-icon-cache", "-f", "-t", str(paths.hicolor_dir)],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
