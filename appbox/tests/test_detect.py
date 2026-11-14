import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from appbox import AppPaths, get_paths
from appbox.detect import (
    UnsupportedPackageError,
    detect_package_type,
    infer_app_id,
    slugify_app_id,
)


class DetectTests(unittest.TestCase):
    def test_get_paths_uses_home_and_xdg_data_home(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg_data"
            with mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}):
                paths = get_paths()
            self.assertIsInstance(paths, AppPaths)
            self.assertEqual(paths.applications_dir, home / "Applications")
            self.assertEqual(paths.local_bin_dir, home / ".local" / "bin")
            self.assertEqual(paths.desktop_dir, xdg / "applications")
            self.assertEqual(paths.hicolor_dir, xdg / "icons" / "hicolor")
            self.assertEqual(paths.appbox_apps_dir, xdg / "appbox" / "apps")
            self.assertEqual(paths.manifests_dir, xdg / "appbox" / "manifests")

    def test_detect_rpm_magic_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp) / "mystery.bin"
            pkg.write_bytes(b"\xed\xab\xee\xdb" + b"\x00" * 32)
            self.assertEqual(detect_package_type(pkg), "rpm")

    def test_detect_appimage_elf_and_signature(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp) / "app.bin"
            pkg.write_bytes(b"\x7fELF" + b"\x00" * 4 + b"AI\x02" + b"\x00" * 20)
            self.assertEqual(detect_package_type(pkg), "appimage")

    def test_detect_appimage_elf_with_extension(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp) / "T3-Code-0.1.9-x86_64.AppImage"
            pkg.write_bytes(b"\x7fELF" + b"\x00" * 20)
            self.assertEqual(detect_package_type(pkg), "appimage")

    def test_detect_rejects_unsupported_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp) / "notes.txt"
            pkg.write_text("hello", encoding="utf-8")
            with self.assertRaises(UnsupportedPackageError):
                detect_package_type(pkg)

    def test_slugify_and_infer_app_id(self) -> None:
        self.assertEqual(slugify_app_id("T3 Code (Beta)"), "t3-code-beta")
        self.assertEqual(
            infer_app_id(Path("/tmp/T3-Code-0.1.9-x86_64.AppImage")),
            "t3-code",
        )
        self.assertEqual(
            infer_app_id(Path("/tmp/chatgpt.x86_64.rpm")),
            "chatgpt",
        )
        self.assertEqual(
            infer_app_id(Path("/tmp/docker-desktop-x86_64.rpm")),
            "docker-desktop",
        )
        self.assertEqual(
            infer_app_id(Path("/tmp/T3-Code-0.1.9-x86_64.AppImage"), preferred_name="Custom T3"),
            "custom-t3",
        )


if __name__ == "__main__":
    unittest.main()
