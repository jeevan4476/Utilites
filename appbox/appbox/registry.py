"""Manifest persistence and deterministic uninstall/cleanup."""

from __future__ import annotations

import json
import shutil
from dataclasses import asdict, dataclass
from pathlib import Path

from appbox import AppPaths
from appbox.desktop import refresh_desktop_caches


class AppNotFoundError(KeyError):
    """Raised when an installed app ID does not exist in the registry."""


@dataclass(frozen=True)
class AppManifest:
    """Recorded installation metadata for a single appbox-managed app."""

    app_id: str
    name: str
    version: str
    package_type: str
    install_mode: str
    source_file: str
    installed_at: str
    launch_target: str
    wrapper_path: str
    bin_symlink: str | None
    desktop_files: list[str]
    icon_files: list[str]
    app_dir: str
    warnings: list[str]

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> AppManifest:
        return cls(
            app_id=data["app_id"],
            name=data["name"],
            version=data.get("version", "unknown"),
            package_type=data["package_type"],
            install_mode=data["install_mode"],
            source_file=data["source_file"],
            installed_at=data["installed_at"],
            launch_target=data["launch_target"],
            wrapper_path=data["wrapper_path"],
            bin_symlink=data.get("bin_symlink"),
            desktop_files=list(data.get("desktop_files", [])),
            icon_files=list(data.get("icon_files", [])),
            app_dir=data["app_dir"],
            warnings=list(data.get("warnings", [])),
        )


def _manifest_path(app_id: str, paths: AppPaths) -> Path:
    return paths.manifests_dir / f"{app_id}.json"


def save_manifest(manifest: AppManifest, paths: AppPaths) -> Path:
    """Persist *manifest* atomically to the appbox manifests directory."""
    paths.manifests_dir.mkdir(parents=True, exist_ok=True)
    target = _manifest_path(manifest.app_id, paths)
    tmp_path = target.with_suffix(".json.tmp")
    tmp_path.write_text(json.dumps(manifest.to_dict(), indent=2) + "\n", encoding="utf-8")
    tmp_path.replace(target)
    return target


def load_manifest(app_id: str, paths: AppPaths) -> AppManifest | None:
    """Load the manifest for *app_id*, or None if not installed."""
    target = _manifest_path(app_id, paths)
    if not target.is_file():
        return None
    data = json.loads(target.read_text(encoding="utf-8"))
    return AppManifest.from_dict(data)


def list_manifests(paths: AppPaths) -> list[AppManifest]:
    """Return all installed manifests sorted by app_id."""
    if not paths.manifests_dir.is_dir():
        return []
    manifests: list[AppManifest] = []
    for file_path in sorted(paths.manifests_dir.glob("*.json")):
        data = json.loads(file_path.read_text(encoding="utf-8"))
        manifests.append(AppManifest.from_dict(data))
    return manifests


def _unlink_if_present(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)


def remove_installed_app(
    app_id: str,
    paths: AppPaths,
    refresh: bool = True,
) -> AppManifest:
    """Remove all managed artifacts for *app_id* and delete its manifest."""
    manifest = load_manifest(app_id, paths)
    if manifest is None:
        raise AppNotFoundError(f"App '{app_id}' is not installed.")

    for desktop in manifest.desktop_files:
        _unlink_if_present(Path(desktop))

    for icon in manifest.icon_files:
        _unlink_if_present(Path(icon))

    if manifest.bin_symlink:
        _unlink_if_present(Path(manifest.bin_symlink))

    launch_path = Path(manifest.launch_target)
    if launch_path.is_file() and launch_path.resolve().parent == paths.applications_dir.resolve():
        _unlink_if_present(launch_path)

    app_dir = Path(manifest.app_dir)
    if app_dir.is_dir() and app_dir.resolve().parent == paths.appbox_apps_dir.resolve():
        shutil.rmtree(app_dir, ignore_errors=True)

    _unlink_if_present(_manifest_path(app_id, paths))

    if refresh:
        refresh_desktop_caches(paths)

    return manifest
