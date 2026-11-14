"""User-space AppImage and RPM desktop integrator."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

__version__ = "0.1.0"


@dataclass(frozen=True)
class AppPaths:
    """Resolved XDG and user-space directories used by appbox."""

    home: Path
    xdg_data_home: Path
    applications_dir: Path
    local_bin_dir: Path
    desktop_dir: Path
    hicolor_dir: Path
    appbox_data_dir: Path
    appbox_apps_dir: Path
    manifests_dir: Path
    nautilus_scripts_dir: Path


def get_paths() -> AppPaths:
    """Resolve user-space directories dynamically from HOME and XDG_DATA_HOME."""
    home = Path(os.environ.get("HOME", str(Path.home()))).expanduser()
    xdg_data_env = os.environ.get("XDG_DATA_HOME")
    xdg_data_home = (
        Path(xdg_data_env).expanduser()
        if xdg_data_env
        else home / ".local" / "share"
    )
    appbox_data_dir = xdg_data_home / "appbox"
    return AppPaths(
        home=home,
        xdg_data_home=xdg_data_home,
        applications_dir=home / "Applications",
        local_bin_dir=home / ".local" / "bin",
        desktop_dir=xdg_data_home / "applications",
        hicolor_dir=xdg_data_home / "icons" / "hicolor",
        appbox_data_dir=appbox_data_dir,
        appbox_apps_dir=appbox_data_dir / "apps",
        manifests_dir=appbox_data_dir / "manifests",
        nautilus_scripts_dir=xdg_data_home / "nautilus" / "scripts",
    )
