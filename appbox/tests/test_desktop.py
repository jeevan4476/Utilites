import struct
import tempfile
import unittest
from pathlib import Path

from appbox.desktop import (
    install_best_icons,
    parse_desktop_metadata,
    rewrite_desktop_content,
)


def make_png_bytes(width: int, height: int) -> bytes:
    ihdr_data = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + ihdr_data + b"\x00" * 4


class DesktopTests(unittest.TestCase):
    def test_parse_desktop_metadata_handles_quotes_and_field_codes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            desktop = Path(tmp) / "t3.desktop"
            desktop.write_text(
                "[Desktop Entry]\n"
                "Type=Application\n"
                "Name=T3 Code\n"
                'Exec="/opt/T3 Code/t3-code" --enable-features=Wayland %U\n'
                "Icon=t3-code\n"
                "Categories=Development;IDE;\n",
                encoding="utf-8",
            )
            meta = parse_desktop_metadata(desktop)
            self.assertEqual(meta.name, "T3 Code")
            self.assertEqual(meta.exec_cmd, "/opt/T3 Code/t3-code")
            self.assertEqual(meta.exec_args, ["--enable-features=Wayland"])
            self.assertEqual(meta.icon_name, "t3-code")
            self.assertEqual(meta.field_codes, ["%U"])

    def test_rewrite_desktop_content_preserves_actions_and_updates_exec_icon(self) -> None:
        original = (
            "[Desktop Entry]\n"
            "Type=Application\n"
            "Name=My App\n"
            "TryExec=/opt/myapp/myapp\n"
            "Exec=/opt/myapp/myapp --flag %F\n"
            "Icon=/opt/myapp/icon.png\n"
            "Categories=Utility;\n"
            "\n"
            "[Desktop Action new-window]\n"
            "Name=New Window\n"
            "Exec=/opt/myapp/myapp --new-window\n"
            "Icon=myapp\n"
        )
        rewritten = rewrite_desktop_content(
            original,
            app_id="my-app",
            exec_target=Path("/home/user/.local/share/appbox/apps/my-app/launch.sh"),
            icon_ref="my-app",
            source_name="MyApp-1.0.rpm",
        )
        self.assertIn(
            'Exec="/home/user/.local/share/appbox/apps/my-app/launch.sh" %F',
            rewritten,
        )
        self.assertIn(
            'TryExec=/home/user/.local/share/appbox/apps/my-app/launch.sh',
            rewritten,
        )
        self.assertIn("Icon=my-app", rewritten)
        self.assertIn("X-AppBox-Id=my-app", rewritten)
        self.assertIn("X-AppBox-Source=MyApp-1.0.rpm", rewritten)
        self.assertIn("[Desktop Action new-window]", rewritten)
        self.assertIn(
            'Exec="/home/user/.local/share/appbox/apps/my-app/launch.sh" --new-window',
            rewritten,
        )

    def test_install_best_icons_detects_png_size_and_svg(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            src_dir = Path(tmp) / "src"
            src_dir.mkdir()
            png_small = src_dir / "small.png"
            png_small.write_bytes(make_png_bytes(48, 48))
            dir_icon = src_dir / ".DirIcon"
            dir_icon.write_bytes(make_png_bytes(256, 256))
            svg_icon = src_dir / "app.svg"
            svg_icon.write_text("<svg></svg>", encoding="utf-8")

            hicolor = Path(tmp) / "hicolor"
            installed = install_best_icons(
                [png_small, dir_icon, svg_icon],
                app_id="t3-code",
                hicolor_dir=hicolor,
            )
            expected_paths = {
                hicolor / "48x48" / "apps" / "t3-code.png",
                hicolor / "256x256" / "apps" / "t3-code.png",
                hicolor / "scalable" / "apps" / "t3-code.svg",
            }
            self.assertEqual(set(installed), expected_paths)
            for path in expected_paths:
                self.assertTrue(path.is_file())


if __name__ == "__main__":
    unittest.main()
