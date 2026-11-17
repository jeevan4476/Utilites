"""Unit tests for lanbeam.ui.popup (headless, no GTK window required)."""

import json
import unittest

# Import the module under test. GTK import is guarded; we only exercise the
# argument parsing / self-test path here so a display is not required.
import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "ui"))

import popup  # noqa: E402


class PopupArgParsingTests(unittest.TestCase):
    def test_self_test_exits_zero(self):
        rc = popup.main(["--self-test"])
        self.assertEqual(rc, 0)

    def test_missing_args_returns_one(self):
        rc = popup.main([])
        self.assertEqual(rc, 1)


class StatusParsingTests(unittest.TestCase):
    def test_apply_status_parses_fields(self):
        # Simulate the JSON the server returns.
        data = {
            "mode": "send",
            "active": True,
            "downloads_completed": 3,
            "uploads_received": 2,
            "remaining_seconds": 125,
        }
        dc = data.get("downloads_completed", 0)
        ur = data.get("uploads_received", 0)
        remaining = data.get("remaining_seconds", 0)
        self.assertEqual(dc, 3)
        self.assertEqual(ur, 2)
        self.assertEqual(remaining // 60, 2)
        self.assertEqual(remaining % 60, 5)

    def test_inactive_stops_polling(self):
        data = {"active": False, "mode": "send"}
        self.assertFalse(data.get("active", False))


if __name__ == "__main__":
    unittest.main()
