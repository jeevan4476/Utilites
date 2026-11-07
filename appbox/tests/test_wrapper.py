import stat
import tempfile
import unittest
from pathlib import Path

from appbox.wrapper import (
    find_chrome_sandbox,
    generate_wrapper_script,
    has_suid_sandbox,
    write_wrapper_script,
)


class WrapperTests(unittest.TestCase):
    def test_portable_wrapper_script(self) -> None:
        target = Path("/home/user/Applications/t3-code.AppImage")
        script = generate_wrapper_script(
            target_exec=target,
            root_dir=None,
            default_args=["--enable-features=UseOzonePlatform"],
            mode="portable",
        )
        self.assertIn("#!/usr/bin/env bash", script)
        self.assertIn('exec "/home/user/Applications/t3-code.AppImage"', script)
        self.assertIn("--enable-features=UseOzonePlatform", script)
        self.assertIn('"$@"', script)

    def test_extracted_wrapper_sets_paths_and_no_sandbox_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            bin_dir = root / "opt" / "MyApp"
            lib_dir = root / "usr" / "lib64"
            usr_bin = root / "usr" / "bin"
            share_dir = root / "usr" / "share"
            bin_dir.mkdir(parents=True)
            lib_dir.mkdir(parents=True)
            usr_bin.mkdir(parents=True)
            share_dir.mkdir(parents=True)

            exe = bin_dir / "myapp"
            exe.write_text("#!/bin/sh\n", encoding="utf-8")
            sandbox = bin_dir / "chrome-sandbox"
            sandbox.write_bytes(b"\x7fELF")
            sandbox.chmod(0o755)

            self.assertEqual(find_chrome_sandbox(root), sandbox)
            self.assertFalse(has_suid_sandbox(sandbox))

            script = generate_wrapper_script(
                target_exec=exe,
                root_dir=root,
                default_args=[],
                mode="extracted",
            )
            self.assertIn("LD_LIBRARY_PATH", script)
            self.assertIn(str(lib_dir), script)
            self.assertIn(str(bin_dir), script)
            self.assertIn(str(usr_bin), script)
            self.assertIn(str(share_dir), script)
            self.assertIn("--no-sandbox", script)

            dest = Path(tmp) / "launch.sh"
            write_wrapper_script(dest, script)
            self.assertTrue(dest.is_file())
            self.assertTrue(bool(dest.stat().st_mode & stat.S_IXUSR))


if __name__ == "__main__":
    unittest.main()
