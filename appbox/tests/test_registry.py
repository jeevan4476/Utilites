import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from appbox import get_paths
from appbox.registry import (
    AppManifest,
    AppNotFoundError,
    list_manifests,
    load_manifest,
    remove_installed_app,
    save_manifest,
)


class RegistryTests(unittest.TestCase):
    def test_save_load_list_and_remove_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg"
            with mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}):
                paths = get_paths()

                app_dir = paths.appbox_apps_dir / "t3-code"
                app_dir.mkdir(parents=True)
                wrapper = app_dir / "launch.sh"
                wrapper.write_text("#!/bin/sh\n", encoding="utf-8")

                paths.applications_dir.mkdir(parents=True)
                portable_bin = paths.applications_dir / "t3-code.AppImage"
                portable_bin.write_bytes(b"appimage")

                paths.desktop_dir.mkdir(parents=True)
                desktop_file = paths.desktop_dir / "appbox-t3-code.desktop"
                desktop_file.write_text("[Desktop Entry]\n", encoding="utf-8")

                icon_file = paths.hicolor_dir / "256x256" / "apps" / "t3-code.png"
                icon_file.parent.mkdir(parents=True)
                icon_file.write_bytes(b"png")

                paths.local_bin_dir.mkdir(parents=True)
                bin_link = paths.local_bin_dir / "t3-code"
                bin_link.symlink_to(wrapper)

                unrelated = paths.desktop_dir / "other.desktop"
                unrelated.write_text("[Desktop Entry]\n", encoding="utf-8")

                manifest = AppManifest(
                    app_id="t3-code",
                    name="T3 Code",
                    version="0.1.9",
                    package_type="appimage",
                    install_mode="portable",
                    source_file="/tmp/T3-Code-0.1.9-x86_64.AppImage",
                    installed_at="2026-11-14T18:30:00+05:30",
                    launch_target=str(portable_bin),
                    wrapper_path=str(wrapper),
                    bin_symlink=str(bin_link),
                    desktop_files=[str(desktop_file)],
                    icon_files=[str(icon_file)],
                    app_dir=str(app_dir),
                    warnings=[],
                )
                save_manifest(manifest, paths)

                loaded = load_manifest("t3-code", paths)
                self.assertEqual(loaded, manifest)
                self.assertEqual(list_manifests(paths), [manifest])

                removed = remove_installed_app("t3-code", paths, refresh=False)
                self.assertEqual(removed.app_id, "t3-code")
                self.assertFalse(desktop_file.exists())
                self.assertFalse(icon_file.exists())
                self.assertFalse(bin_link.exists() or bin_link.is_symlink())
                self.assertFalse(portable_bin.exists())
                self.assertFalse(app_dir.exists())
                self.assertIsNone(load_manifest("t3-code", paths))
                self.assertTrue(unrelated.exists())

                with self.assertRaises(AppNotFoundError):
                    remove_installed_app("t3-code", paths, refresh=False)


if __name__ == "__main__":
    unittest.main()
