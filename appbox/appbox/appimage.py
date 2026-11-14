"""AppImage metadata extraction and installation."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from appbox import AppPaths
from appbox.desktop import parse_desktop_metadata, pick_primary_desktop
from appbox.detect import infer_app_id
from appbox.integrator import finalize_install, safe_copy_file
from appbox.registry import AppManifest, load_manifest, remove_installed_app

_FILENAME_VERSION_RE = re.compile(r"[-_](\d+(?:\.\d+)+)")


def _extract_with_7z(appimage_path: Path, work_dir: Path) -> Path | None:
    """Try extracting .desktop and icon assets safely with 7z without executing."""
    seven_z = shutil.which("7z") or shutil.which("7zz")
    if not seven_z:
        return None

    meta_dir = work_dir / "meta"
    meta_dir.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [
            seven_z,
            "x",
            "-y",
            f"-o{meta_dir}",
            str(appimage_path),
            "*.desktop",
            ".DirIcon",
            "usr/share/icons/*",
            "*.png",
            "*.svg",
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if proc.returncode != 0:
        return None
    has_desktop = any(meta_dir.rglob("*.desktop"))
    if not has_desktop:
        return None
    return meta_dir


def _extract_with_appimage_flag(appimage_path: Path, work_dir: Path) -> Path:
    """Unpack an AppImage using its built-in --appimage-extract flag."""
    runner = work_dir / "extract_target.AppImage"
    shutil.copy2(appimage_path, runner)
    runner.chmod(0o755)
    env = os.environ.copy()
    env.pop("APPIMAGE_EXTRACT_AND_RUN", None)
    proc = subprocess.run(
        [str(runner), "--appimage-extract"],
        cwd=work_dir,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    squashfs_root = work_dir / "squashfs-root"
    if proc.returncode != 0 or not squashfs_root.is_dir():
        detail = (proc.stderr or proc.stdout).strip()
        raise RuntimeError(f"Failed to extract AppImage '{appimage_path.name}': {detail}")
    return squashfs_root


def _extract_appimage_version(desktop_path: Path | None, source_path: Path) -> str:
    if desktop_path is not None:
        for line in desktop_path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("X-AppImage-Version="):
                val = line.split("=", 1)[1].strip()
                if val:
                    return val
    match = _FILENAME_VERSION_RE.search(source_path.stem)
    return match.group(1) if match else "unknown"


def install_appimage(
    source_path: Path,
    paths: AppPaths,
    app_id: str | None = None,
    extract: bool = False,
    move: bool = False,
) -> AppManifest:
    """Install an AppImage in portable (~/Applications) or extracted mode."""
    if not source_path.is_file():
        raise FileNotFoundError(f"AppImage not found: {source_path}")

    with tempfile.TemporaryDirectory(prefix="appbox-ai-") as tmp:
        work_dir = Path(tmp)
        extracted_tree: Path | None = None
        if not extract:
            extracted_tree = _extract_with_7z(source_path, work_dir)
        if extracted_tree is None:
            extracted_tree = _extract_with_appimage_flag(source_path, work_dir)

        desktop_files = list(extracted_tree.rglob("*.desktop"))
        primary_desktop = pick_primary_desktop(desktop_files)
        desktop_meta = parse_desktop_metadata(primary_desktop) if primary_desktop else None

        resolved_id = infer_app_id(
            source_path,
            preferred_name=app_id or (desktop_meta.name if desktop_meta else None),
        )
        display_name = desktop_meta.name if desktop_meta else resolved_id
        version = _extract_appimage_version(primary_desktop, source_path)

        copy_source = source_path
        if load_manifest(resolved_id, paths) is not None:
            portable_dest = paths.applications_dir / f"{resolved_id}.AppImage"
            if portable_dest.exists() and source_path.resolve() == portable_dest.resolve():
                copy_source = work_dir / "source_backup.AppImage"
                shutil.copy2(source_path, copy_source)
            remove_installed_app(resolved_id, paths, refresh=False)

        app_dir = paths.appbox_apps_dir / resolved_id
        app_dir.mkdir(parents=True, exist_ok=True)

        if extract:
            dest_root = app_dir / "root"
            if dest_root.exists():
                shutil.rmtree(dest_root)
            shutil.move(str(extracted_tree), str(dest_root))
            apprun = dest_root / "AppRun"
            if not apprun.exists():
                raise RuntimeError("Extracted AppImage does not contain an AppRun entry point.")
            apprun.chmod(0o755)
            launch_target = apprun
            metadata_root = dest_root
            install_mode = "extracted"
            extracted_root = dest_root
            # Re-resolve primary_desktop path inside moved dest_root.
            if primary_desktop is not None:
                rel = primary_desktop.relative_to(extracted_tree)
                primary_desktop = dest_root / rel
        else:
            launch_target = paths.applications_dir / f"{resolved_id}.AppImage"
            safe_copy_file(copy_source, launch_target)
            launch_target.chmod(0o755)
            metadata_root = extracted_tree
            install_mode = "portable"
            extracted_root = None

        default_args = (
            [arg for arg in desktop_meta.exec_args if arg]
            if desktop_meta
            else []
        )
        return finalize_install(
            app_id=resolved_id,
            display_name=display_name,
            version=version,
            package_type="appimage",
            install_mode=install_mode,
            source_path=source_path,
            paths=paths,
            app_dir=app_dir,
            launch_target=launch_target,
            extracted_root=extracted_root,
            metadata_root=metadata_root,
            desktop_source=primary_desktop,
            default_args=default_args,
            warnings=[],
            move_source=move,
        )
