"""Shared integration pipeline and package installation dispatcher."""

from __future__ import annotations

import shutil
from datetime import datetime
from pathlib import Path

from appbox import AppPaths
from appbox.desktop import (
    find_icon_candidates,
    install_best_icons,
    parse_desktop_metadata,
    refresh_desktop_caches,
    rewrite_desktop_content,
)
from appbox.detect import PackageType, detect_package_type, infer_app_id
from appbox.registry import AppManifest, save_manifest
from appbox.wrapper import InstallMode, generate_wrapper_script, write_wrapper_script


def finalize_install(
    *,
    app_id: str,
    display_name: str,
    version: str,
    package_type: PackageType,
    install_mode: InstallMode,
    source_path: Path,
    paths: AppPaths,
    app_dir: Path,
    launch_target: Path,
    extracted_root: Path | None,
    metadata_root: Path,
    desktop_source: Path | None,
    default_args: list[str],
    warnings: list[str],
    move_source: bool,
) -> AppManifest:
    """Commit desktop entry, icons, wrapper script, CLI symlink, and manifest."""
    wrapper_path = app_dir / "launch.sh"
    wrapper_script = generate_wrapper_script(
        target_exec=launch_target,
        root_dir=extracted_root,
        default_args=default_args,
        mode=install_mode,
    )
    write_wrapper_script(wrapper_path, wrapper_script)

    icon_hint = None
    if desktop_source is not None:
        icon_hint = parse_desktop_metadata(desktop_source).icon_name
    icon_candidates = find_icon_candidates(metadata_root, icon_hint=icon_hint)
    installed_icons = install_best_icons(icon_candidates, app_id=app_id, hicolor_dir=paths.hicolor_dir)

    desktop_files: list[str] = []
    if desktop_source is not None:
        icon_ref = app_id if installed_icons else (icon_hint or "application-x-executable")
        raw_desktop = desktop_source.read_text(encoding="utf-8", errors="replace")
        rewritten = rewrite_desktop_content(
            raw_desktop,
            app_id=app_id,
            exec_target=wrapper_path,
            icon_ref=icon_ref,
            source_name=source_path.name,
        )
        paths.desktop_dir.mkdir(parents=True, exist_ok=True)
        dest_desktop = paths.desktop_dir / f"appbox-{app_id}.desktop"
        dest_desktop.write_text(rewritten, encoding="utf-8")
        dest_desktop.chmod(0o644)
        desktop_files.append(str(dest_desktop))

    paths.local_bin_dir.mkdir(parents=True, exist_ok=True)
    bin_symlink = paths.local_bin_dir / app_id
    if bin_symlink.is_symlink() or bin_symlink.exists():
        bin_symlink.unlink()
    bin_symlink.symlink_to(wrapper_path)

    manifest = AppManifest(
        app_id=app_id,
        name=display_name,
        version=version,
        package_type=package_type,
        install_mode=install_mode,
        source_file=str(source_path.resolve()),
        installed_at=datetime.now().astimezone().isoformat(timespec="seconds"),
        launch_target=str(launch_target),
        wrapper_path=str(wrapper_path),
        bin_symlink=str(bin_symlink),
        desktop_files=desktop_files,
        icon_files=[str(p) for p in installed_icons],
        app_dir=str(app_dir),
        warnings=warnings,
    )
    save_manifest(manifest, paths)
    refresh_desktop_caches(paths)

    if move_source and source_path.exists() and source_path.resolve() != launch_target.resolve():
        source_path.unlink()

    return manifest


def install_package(
    source_path: Path,
    paths: AppPaths,
    app_id: str | None = None,
    extract: bool = False,
    move: bool = False,
) -> AppManifest:
    """Detect package format and install *source_path* into user space."""
    from appbox.appimage import install_appimage
    from appbox.rpm import install_rpm

    pkg_type = detect_package_type(source_path)
    resolved_id = infer_app_id(source_path, preferred_name=app_id) if app_id else None
    if pkg_type == "appimage":
        return install_appimage(
            source_path,
            paths,
            app_id=resolved_id,
            extract=extract,
            move=move,
        )
    return install_rpm(source_path, paths, app_id=resolved_id, move=move)


def safe_copy_file(src: Path, dest: Path) -> None:
    """Copy *src* to *dest* atomically unless both resolve to the same file."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists() and src.resolve() == dest.resolve():
        return
    tmp_dest = dest.with_name(f".{dest.name}.tmp")
    shutil.copy2(src, tmp_dest)
    tmp_dest.replace(dest)
