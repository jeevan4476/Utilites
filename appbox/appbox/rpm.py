"""Rootless RPM extraction and user-space app integration."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from appbox import AppPaths
from appbox.desktop import parse_desktop_metadata, pick_primary_desktop
from appbox.detect import infer_app_id
from appbox.integrator import finalize_install
from appbox.registry import AppManifest, load_manifest, remove_installed_app

_SYSTEM_HOOK_DIRS = (
    ("lib/systemd/system", "systemd unit files"),
    ("usr/lib/systemd/system", "systemd unit files"),
    ("etc/systemd", "systemd configuration"),
    ("etc/udev/rules.d", "udev rules"),
    ("usr/lib/udev/rules.d", "udev rules"),
    ("usr/lib/modules", "kernel modules"),
)


def _query_rpm_metadata(rpm_path: Path) -> tuple[str | None, str]:
    """Query package name and version via `rpm -qp`, returning (name, version)."""
    rpm_bin = shutil.which("rpm")
    if not rpm_bin:
        return None, "unknown"
    proc = subprocess.run(
        [rpm_bin, "-qp", "--queryformat", "%{NAME}\n%{VERSION}-%{RELEASE}\n", str(rpm_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        return None, "unknown"
    lines = [line.strip() for line in proc.stdout.splitlines() if line.strip()]
    if len(lines) >= 2:
        return lines[0], lines[1]
    if len(lines) == 1:
        return lines[0], "unknown"
    return None, "unknown"


def _unpack_rpm(rpm_path: Path, root_dir: Path) -> None:
    """Extract *rpm_path* into *root_dir* without root using rpm2cpio + cpio."""
    rpm2cpio_bin = shutil.which("rpm2cpio")
    cpio_bin = shutil.which("cpio")
    if not rpm2cpio_bin or not cpio_bin:
        raise RuntimeError(
            "RPM extraction requires 'rpm2cpio' and 'cpio' (install via: sudo dnf install rpm cpio)."
        )

    root_dir.mkdir(parents=True, exist_ok=True)
    rpm_proc = subprocess.Popen(
        [rpm2cpio_bin, str(rpm_path)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    cpio_proc = subprocess.Popen(
        [cpio_bin, "-idmu", "--no-absolute-filenames", "--quiet"],
        cwd=root_dir,
        stdin=rpm_proc.stdout,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if rpm_proc.stdout is not None:
        rpm_proc.stdout.close()

    _, cpio_err = cpio_proc.communicate()
    _, rpm_err = rpm_proc.communicate()
    if rpm_proc.returncode != 0 or cpio_proc.returncode != 0:
        detail = (cpio_err or rpm_err).decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"Failed to unpack RPM '{rpm_path.name}': {detail}")


def detect_rpm_warnings(root_dir: Path) -> list[str]:
    """Detect system-level hooks in an extracted RPM that won't run in user space."""
    warnings: list[str] = []
    seen_labels: set[str] = set()
    for rel_path, label in _SYSTEM_HOOK_DIRS:
        candidate = root_dir / rel_path
        if candidate.exists() and any(candidate.iterdir()):
            if label not in seen_labels:
                seen_labels.add(label)
                warnings.append(
                    f"Package ships {label} under /{rel_path}; these will not activate in user-space. "
                    "Use `sudo dnf install` if system services or drivers are required."
                )
    return warnings


def _resolve_symlink_within_root(root_dir: Path, candidate: Path) -> Path:
    """Resolve symlinks inside *root_dir*, mapping absolute symlink targets back into *root_dir*."""
    current = candidate
    for _ in range(8):
        if not current.is_symlink():
            return current
        raw_target = Path(os.readlink(current))
        if raw_target.is_absolute():
            current = root_dir / str(raw_target).lstrip("/")
        else:
            current = (current.parent / raw_target).resolve()
    return current


def resolve_rpm_executable(root_dir: Path, exec_cmd: str) -> Path | None:
    """Locate the target executable inside an extracted RPM tree."""
    candidates: list[Path] = []
    if exec_cmd:
        cleaned = exec_cmd.strip()
        if cleaned.startswith("/"):
            candidates.append(root_dir / cleaned.lstrip("/"))
        else:
            cmd_name = Path(cleaned).name
            candidates.extend(
                [
                    root_dir / "usr" / "bin" / cmd_name,
                    root_dir / "usr" / "local" / "bin" / cmd_name,
                ]
            )
            candidates.extend(root_dir.rglob(cmd_name))

    for candidate in candidates:
        resolved = _resolve_symlink_within_root(root_dir, candidate)
        if resolved.is_file() and os.access(resolved, os.X_OK):
            return resolved

    # Fallback: pick first executable in usr/bin or opt.
    for search_dir in (root_dir / "usr" / "bin", root_dir / "usr" / "local" / "bin", root_dir / "opt"):
        if not search_dir.is_dir():
            continue
        for item in sorted(search_dir.rglob("*")):
            resolved = _resolve_symlink_within_root(root_dir, item)
            if resolved.is_file() and os.access(resolved, os.X_OK):
                return resolved
    return None


def install_rpm(
    source_path: Path,
    paths: AppPaths,
    app_id: str | None = None,
    move: bool = False,
) -> AppManifest:
    """Extract an RPM into ~/.local/share/appbox/apps/<id>/root and integrate it."""
    if not source_path.is_file():
        raise FileNotFoundError(f"RPM package not found: {source_path}")

    rpm_name, rpm_version = _query_rpm_metadata(source_path)

    with tempfile.TemporaryDirectory(prefix="appbox-rpm-") as tmp:
        stage_root = Path(tmp) / "root"
        _unpack_rpm(source_path, stage_root)

        desktop_files = list((stage_root / "usr" / "share" / "applications").glob("*.desktop"))
        if not desktop_files:
            desktop_files = list(stage_root.rglob("*.desktop"))
        primary_desktop = pick_primary_desktop(desktop_files)
        desktop_meta = parse_desktop_metadata(primary_desktop) if primary_desktop else None

        resolved_id = infer_app_id(
            source_path,
            preferred_name=app_id or (desktop_meta.name if desktop_meta else rpm_name),
        )
        display_name = desktop_meta.name if desktop_meta else (rpm_name or resolved_id)

        if load_manifest(resolved_id, paths) is not None:
            remove_installed_app(resolved_id, paths, refresh=False)

        app_dir = paths.appbox_apps_dir / resolved_id
        app_dir.mkdir(parents=True, exist_ok=True)
        dest_root = app_dir / "root"
        if dest_root.exists():
            shutil.rmtree(dest_root)
        shutil.move(str(stage_root), str(dest_root))

        if primary_desktop is not None:
            rel_desktop = primary_desktop.relative_to(stage_root)
            primary_desktop = dest_root / rel_desktop

        exec_hint = desktop_meta.exec_cmd if desktop_meta else (rpm_name or resolved_id)
        launch_target = resolve_rpm_executable(dest_root, exec_hint)
        if launch_target is None:
            raise RuntimeError(
                f"Could not find an executable binary inside '{source_path.name}'."
            )

        warnings = detect_rpm_warnings(dest_root)
        default_args = desktop_meta.exec_args if desktop_meta else []
        return finalize_install(
            app_id=resolved_id,
            display_name=display_name,
            version=rpm_version,
            package_type="rpm",
            install_mode="extracted",
            source_path=source_path,
            paths=paths,
            app_dir=app_dir,
            launch_target=launch_target,
            extracted_root=dest_root,
            metadata_root=dest_root,
            desktop_source=primary_desktop,
            default_args=default_args,
            warnings=warnings,
            move_source=move,
        )
