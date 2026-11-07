import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

from appbox import get_paths
from appbox.cli import main
from appbox.registry import AppManifest, save_manifest


class CliTests(unittest.TestCase):
    def test_list_json_info_and_remove(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg"
            with (
                mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}),
                mock.patch("appbox.registry.refresh_desktop_caches"),
            ):
                paths = get_paths()
                manifest = AppManifest(
                    app_id="demo-app",
                    name="Demo App",
                    version="1.0.0",
                    package_type="appimage",
                    install_mode="portable",
                    source_file="/tmp/Demo-1.0.0.AppImage",
                    installed_at="2026-11-14T18:30:00+05:30",
                    launch_target=str(paths.applications_dir / "demo-app.AppImage"),
                    wrapper_path=str(paths.appbox_apps_dir / "demo-app" / "launch.sh"),
                    bin_symlink=str(paths.local_bin_dir / "demo-app"),
                    desktop_files=[],
                    icon_files=[],
                    app_dir=str(paths.appbox_apps_dir / "demo-app"),
                    warnings=[],
                )
                save_manifest(manifest, paths)

                out = io.StringIO()
                with redirect_stdout(out):
                    code = main(["list", "--json"])
                self.assertEqual(code, 0)
                parsed = json.loads(out.getvalue())
                self.assertEqual(len(parsed), 1)
                self.assertEqual(parsed[0]["app_id"], "demo-app")

                out_info = io.StringIO()
                with redirect_stdout(out_info):
                    code = main(["info", "demo-app"])
                self.assertEqual(code, 0)
                self.assertIn("Demo App", out_info.getvalue())

                out_rm = io.StringIO()
                with redirect_stdout(out_rm):
                    code = main(["remove", "demo-app"])
                self.assertEqual(code, 0)
                self.assertIn("Removed demo-app", out_rm.getvalue())

    def test_install_missing_file_returns_nonzero(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg"
            with mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}):
                err = io.StringIO()
                with redirect_stderr(err):
                    code = main(["install", str(Path(tmp) / "missing.AppImage")])
                self.assertEqual(code, 1)
                self.assertIn("not found", err.getvalue().lower())

    def test_setup_nautilus_installs_executable_script(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            xdg = Path(tmp) / "xdg"
            with mock.patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(xdg)}):
                out = io.StringIO()
                with redirect_stdout(out):
                    code = main(["setup-nautilus"])
                self.assertEqual(code, 0)
                script = xdg / "nautilus" / "scripts" / "Install as App"
                self.assertTrue(script.is_file())
                self.assertTrue(os.access(script, os.X_OK))


if __name__ == "__main__":
    unittest.main()
