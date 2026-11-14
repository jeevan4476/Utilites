import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from appbox import get_paths
from appbox.appimage import install_appimage
from appbox.integrator import install_package
from appbox.rpm import detect_rpm_warnings, install_rpm, resolve_rpm_executable


def make_png_bytes(width: int = 128, height: int = 128) -> bytes:
    ihdr_data = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + ihdr_data + b"\x00" * 4


class InstallersTests(unittest.TestCase):
    def test_install_appimage_portable_with_extract_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            home = tmp_path / "home"
            xdg = tmp_path / "xdg"
            src_appimage = tmp_path / "T3-Code-0.1.9-x86_64.AppImage"
            src_appimage.write_bytes(b"\x7fELF" + b"\x00" * 4 + b"AI\x02" + b"\x00" * 32)

            def fake_extract(appimage_copy: Path, work_dir: Path) -> Path:
                sq = work_dir / "squashfs-root"
                sq.mkdir(parents=True)
                (sq / "t3-code.desktop").write_text(
                    "[Desktop Entry]\n"
                    "Type=Application\n"
                    "Name=T3 Code\n"
                    "Exec=AppRun --no-sandbox %U\n"
                    "Icon=t3-code\n"
                    "X-AppImage-Version=0.1.9\n",
                    encoding="utf-8",
                )
                (sq / "t3-code.png").write_bytes(make_png_bytes(256, 256))
                apprun = sq / "AppRun"
                apprun.write_text("#!/bin/sh\n", encoding="utf-8")
                apprun.chmod(0o755)
                return sq

            with (
                mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}),
                mock.patch("appbox.appimage._extract_with_7z", return_value=None),
                mock.patch("appbox.appimage._extract_with_appimage_flag", side_effect=fake_extract),
                mock.patch("appbox.integrator.refresh_desktop_caches"),
            ):
                paths = get_paths()
                manifest = install_package(src_appimage, paths)

                self.assertEqual(manifest.app_id, "t3-code")
                self.assertEqual(manifest.version, "0.1.9")
                self.assertEqual(manifest.install_mode, "portable")
                self.assertTrue((paths.applications_dir / "t3-code.AppImage").is_file())
                self.assertTrue((paths.desktop_dir / "appbox-t3-code.desktop").is_file())
                self.assertTrue((paths.local_bin_dir / "t3-code").is_symlink())
                self.assertTrue(
                    (paths.hicolor_dir / "256x256" / "apps" / "t3-code.png").is_file()
                )
                self.assertTrue(src_appimage.exists())

    def test_install_appimage_extracted_mode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            home = tmp_path / "home"
            xdg = tmp_path / "xdg"
            src_appimage = tmp_path / "MyTool-2.0.0-x86_64.AppImage"
            src_appimage.write_bytes(b"\x7fELF" + b"\x00" * 4 + b"AI\x02" + b"\x00" * 32)

            def fake_extract(appimage_copy: Path, work_dir: Path) -> Path:
                sq = work_dir / "squashfs-root"
                sq.mkdir(parents=True)
                (sq / "mytool.desktop").write_text(
                    "[Desktop Entry]\nType=Application\nName=MyTool\nExec=AppRun\nIcon=mytool\n",
                    encoding="utf-8",
                )
                (sq / ".DirIcon").write_bytes(make_png_bytes(128, 128))
                apprun = sq / "AppRun"
                apprun.write_text("#!/bin/sh\n", encoding="utf-8")
                apprun.chmod(0o755)
                return sq

            with (
                mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}),
                mock.patch("appbox.appimage._extract_with_appimage_flag", side_effect=fake_extract),
                mock.patch("appbox.integrator.refresh_desktop_caches"),
            ):
                paths = get_paths()
                manifest = install_appimage(src_appimage, paths, extract=True)
                self.assertEqual(manifest.install_mode, "extracted")
                self.assertTrue((paths.appbox_apps_dir / "mytool" / "root" / "AppRun").is_file())
                self.assertFalse((paths.applications_dir / "mytool.AppImage").exists())

    def test_install_rpm_with_absolute_symlink_sandbox_and_systemd_warning(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            home = tmp_path / "home"
            xdg = tmp_path / "xdg"
            src_rpm = tmp_path / "chatgpt.x86_64.rpm"
            src_rpm.write_bytes(b"\xed\xab\xee\xdb" + b"\x00" * 32)

            def fake_unpack_rpm(rpm_path: Path, root_dir: Path) -> None:
                opt_app = root_dir / "opt" / "ChatGPT"
                opt_app.mkdir(parents=True)
                real_bin = opt_app / "chatgpt"
                real_bin.write_text("#!/bin/sh\n", encoding="utf-8")
                real_bin.chmod(0o755)

                sandbox = opt_app / "chrome-sandbox"
                sandbox.write_bytes(b"\x7fELF")
                sandbox.chmod(0o755)

                usr_bin = root_dir / "usr" / "bin"
                usr_bin.mkdir(parents=True)
                (usr_bin / "chatgpt").symlink_to("/opt/ChatGPT/chatgpt")

                apps_dir = root_dir / "usr" / "share" / "applications"
                apps_dir.mkdir(parents=True)
                (apps_dir / "chatgpt.desktop").write_text(
                    "[Desktop Entry]\n"
                    "Type=Application\n"
                    "Name=ChatGPT\n"
                    "Exec=/usr/bin/chatgpt %U\n"
                    "Icon=chatgpt\n",
                    encoding="utf-8",
                )
                icon_dir = root_dir / "usr" / "share" / "icons" / "hicolor" / "256x256" / "apps"
                icon_dir.mkdir(parents=True)
                (icon_dir / "chatgpt.png").write_bytes(make_png_bytes(256, 256))

                systemd_dir = root_dir / "usr" / "lib" / "systemd" / "system"
                systemd_dir.mkdir(parents=True)
                (systemd_dir / "chatgpt.service").write_text("[Unit]\n", encoding="utf-8")

            with (
                mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}),
                mock.patch("appbox.rpm._unpack_rpm", side_effect=fake_unpack_rpm),
                mock.patch("appbox.rpm._query_rpm_metadata", return_value=("chatgpt", "1.2.3-1")),
                mock.patch("appbox.integrator.refresh_desktop_caches"),
            ):
                paths = get_paths()
                manifest = install_rpm(src_rpm, paths)

                self.assertEqual(manifest.app_id, "chatgpt")
                self.assertEqual(manifest.version, "1.2.3-1")
                self.assertEqual(manifest.install_mode, "extracted")
                self.assertTrue(any("systemd" in w.lower() for w in manifest.warnings))
                wrapper_text = Path(manifest.wrapper_path).read_text(encoding="utf-8")
                self.assertIn("--no-sandbox", wrapper_text)
                self.assertIn("/opt/ChatGPT/chatgpt", wrapper_text)
                self.assertTrue((paths.desktop_dir / "appbox-chatgpt.desktop").is_file())

    def test_detect_rpm_warnings_and_resolve_exec(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            (root / "etc" / "udev" / "rules.d").mkdir(parents=True)
            (root / "etc" / "udev" / "rules.d" / "99-test.rules").write_text("", encoding="utf-8")
            warnings = detect_rpm_warnings(root)
            self.assertEqual(len(warnings), 1)

            opt_dir = root / "opt" / "app"
            opt_dir.mkdir(parents=True)
            exe = opt_dir / "app"
            exe.write_text("#!/bin/sh\n", encoding="utf-8")
            exe.chmod(0o755)
            self.assertEqual(resolve_rpm_executable(root, "/opt/app/app"), exe)


if __name__ == "__main__":
    unittest.main()
