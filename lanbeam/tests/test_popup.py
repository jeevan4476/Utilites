"""Unit tests for lanbeam.ui.popup (headless, no GTK window required)."""

import json
import unittest

# Import the module under test. GTK import is guarded; we only exercise the
# argument parsing / self-test path here so a display is not required.
import sys
import os
from types import SimpleNamespace
from unittest.mock import Mock, patch

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
        status = Mock()
        status.get_text.return_value = "Send • 3 download(s), 2 upload(s)"
        stop = Mock()
        popup.PopupWindow.apply_status(SimpleNamespace(status_label=status, stop_btn=stop), {
            "mode": "send",
            "active": True,
            "downloads_completed": 3,
            "uploads_received": 2,
            "remaining_seconds": 125,
        })
        self.assertEqual(status.set_text.call_args_list[-1].args[0], "Send • 3 download(s), 2 upload(s) • 02:05 left")

    def test_inactive_stops_polling(self):
        status = Mock()
        stop = Mock()
        result = popup.PopupWindow.apply_status(SimpleNamespace(status_label=status, stop_btn=stop), {"active": False, "mode": "send"})
        self.assertFalse(result)
        stop.set_label.assert_called_once_with("Stopped")
        stop.set_sensitive.assert_called_once_with(False)

    def test_status_formatting_uses_bullet(self):
        # Verify the bullet character used in status formatting
        status = Mock()
        status.get_text.return_value = "Send • 3 download(s), 2 upload(s)"
        popup.PopupWindow.apply_status(SimpleNamespace(status_label=status, stop_btn=Mock()), {
            "mode": "send",
            "active": True,
            "downloads_completed": 3,
            "uploads_received": 2,
            "remaining_seconds": 125,
        })
        self.assertIn("•", status.set_text.call_args.args[0])


class QRSVGTests(unittest.TestCase):
    def test_url_normalization_strips_existing_token(self):
        token = "abcdef123456"
        self.assertEqual(popup.PopupWindow.normalize_base_url(f"http://192.168.1.42:5000/{token}/", token), "http://192.168.1.42:5000")

    def test_fetch_qr_uses_normalized_session_url_and_loads_svg(self):
        load_callback = Mock()
        window = SimpleNamespace(base_url="http://192.168.1.42:5000", token="abcdef123456", session_url="http://192.168.1.42:5000/abcdef123456", _load_qr_svg=load_callback)
        picture = Mock()
        loader = Mock()
        loader.get_pixbuf.return_value = object()
        with patch.object(popup.urllib.request, "urlopen") as urlopen, \
             patch.object(popup.GLib, "idle_add") as idle_add, \
             patch.object(popup.GdkPixbuf.PixbufLoader, "new_with_type", return_value=loader):
            urlopen.return_value.__enter__.return_value.read.return_value = b"<svg/>"
            popup.PopupWindow.fetch_qr(window)
            self.assertEqual(urlopen.call_args.args[0], "http://192.168.1.42:5000/abcdef123456/qr.svg")
            callback, svg = idle_add.call_args.args
            self.assertIs(callback, load_callback)
            popup.PopupWindow._load_qr_svg(SimpleNamespace(qr_picture=picture), svg)
            loader.write.assert_called_once_with(b"<svg/>")
            picture.set_pixbuf.assert_called_once_with(loader.get_pixbuf.return_value)


if __name__ == "__main__":
    unittest.main()
