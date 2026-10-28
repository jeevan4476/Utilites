import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from rclip_cli.cli import main


class CliTests(unittest.TestCase):
    def test_list_and_clear_disk_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg"
            store_dir = xdg / "rclip"
            images_dir = store_dir / "images"
            images_dir.mkdir(parents=True)

            img_path = images_dir / "img1.png"
            img_path.write_bytes(b"pngdata")

            history = {
                "version": 2,
                "items": [
                    {
                        "id": "p1",
                        "type": "text",
                        "text": "pinned note",
                        "pinned": True,
                        "timestamp": 200,
                    },
                    {
                        "id": "img1",
                        "type": "image",
                        "imagePath": str(img_path),
                        "width": 800,
                        "height": 600,
                        "byteSize": 7,
                        "pinned": False,
                        "timestamp": 100,
                    },
                ],
            }
            (store_dir / "history.json").write_text(json.dumps(history), encoding="utf-8")

            with mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}):
                out = io.StringIO()
                with redirect_stdout(out):
                    code = main(["list", "--json"])
                self.assertEqual(code, 0)
                items = json.loads(out.getvalue())
                self.assertEqual(len(items), 2)

                # Clear when D-Bus extension is unavailable falls back cleanly to disk
                with mock.patch("rclip_cli.cli._call_dbus", return_value=False):
                    out_clr = io.StringIO()
                    with redirect_stdout(out_clr):
                        code = main(["clear"])
                    self.assertEqual(code, 0)
                self.assertFalse(img_path.exists())
                remaining = json.loads((store_dir / "history.json").read_text(encoding="utf-8"))["items"]
                self.assertEqual(len(remaining), 1)
                self.assertEqual(remaining[0]["id"], "p1")


if __name__ == "__main__":
    unittest.main()
