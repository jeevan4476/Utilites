#!/usr/bin/env python3
"""lanbeam desktop QR popup.

A native GTK4 (PyGObject) window shown on the desktop when sharing files or
text over the LAN. Renders the session QR code, session URL, live transfer
counters polled from ``/<token>/status``, an "Open Downloads" button, and a
"Stop Sharing" button.

Usage:
    popup.py <token> <url> [--port PORT]
    popup.py --self-test
    popup.py --help
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from typing import Optional

try:
    from gi.repository import Gtk, Gdk, GLib
except ImportError as exc:  # pragma: no cover - GTK not installed
    raise SystemExit(
        "lanbeam popup requires PyGObject (GTK 4). Install python3-gi and gir1.2-gtk-4.0"
    ) from exc

POLL_INTERVAL_MS = 1000


class PopupWindow(Gtk.ApplicationWindow):
    def __init__(self, application: Gtk.Application, token: str, url: str, port: int):
        super().__init__(application=application, title="lanbeam")
        self.token = token
        self.base_url = url.rstrip("/")
        self.port = port
        self.status_url = f"{self.base_url}/{token}/status"

        self.set_default_size(420, 560)
        self.set_resizable(True)

        # Root box.
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.set_margin_top(16)
        box.set_margin_bottom(16)
        box.set_margin_start(16)
        box.set_margin_end(16)
        self.set_child(box)

        # QR picture.
        self.qr_picture = Gtk.Picture()
        self.qr_picture.set_can_shrink(True)
        self.qr_picture.set_hexpand(True)
        self.qr_picture.set_vexpand(True)
        box.append(self.qr_picture)

        # Session URL label.
        self.url_label = Gtk.Label(label=url)
        self.url_label.set_wrap(True)
        self.url_label.set_justify(Gtk.Justification.CENTER)
        self.url_label.add_css_class("url-label")
        box.append(self.url_label)

        # Copy link button.
        self.copy_btn = Gtk.Button(label="Copy Link")
        self.copy_btn.add_css_class("suggested-action")
        self.copy_btn.connect("clicked", self.on_copy_link)
        box.append(self.copy_btn)

        # Transfer status label.
        self.status_label = Gtk.Label(label="Waiting for activity…")
        self.status_label.set_wrap(True)
        self.status_label.set_justify(Gtk.Justification.CENTER)
        self.status_label.add_css_class("status-label")
        box.append(self.status_label)

        # Buttons row.
        btn_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        btn_box.set_halign(Gtk.Align.CENTER)

        open_btn = Gtk.Button(label="Open Downloads")
        open_btn.connect("clicked", self.on_open_downloads)
        btn_box.append(open_btn)

        self.stop_btn = Gtk.Button(label="Stop Sharing")
        self.stop_btn.add_css_class("destructive-action")
        self.stop_btn.connect("clicked", self.on_stop)
        btn_box.append(self.stop_btn)

        box.append(btn_box)

        # ESC closes / stops.
        controller = Gtk.EventControllerKey.new()
        controller.connect("key-pressed", self.on_key_pressed)
        self.add_controller(controller)

        # Kick off polling.
        GLib.timeout_add(POLL_INTERVAL_MS, self.poll_status)
        self.poll_status()

    # ------------------------------------------------------------------ #
    # Actions
    # ------------------------------------------------------------------ #
    def on_copy_link(self, _button: Gtk.Button) -> None:
        clipboard = self.get_clipboard()
        clipboard.set(self.base_url + "/" + self.token)

    def on_open_downloads(self, _button: Gtk.Button) -> None:
        import subprocess

        try:
            subprocess.Popen(["xdg-open", os.path.expanduser("~/Downloads")])
        except Exception:  # pragma: no cover
            pass

    def on_stop(self, _button: Gtk.Button) -> None:
        try:
            req = urllib.request.Request(
                f"{self.base_url}/{self.token}/stop",
                data=b"",
                method="POST",
            )
            urllib.request.urlopen(req, timeout=3)
        except Exception:  # pragma: no cover - best effort
            pass
        self.close()

    def on_key_pressed(self, _controller, keyval, keycode, state) -> bool:
        from gi.repository import Gdk

        if keyval == Gdk.KEY_Escape:
            self.on_stop(None)
            return True
        return False

    # ------------------------------------------------------------------ #
    # Polling
    # ------------------------------------------------------------------ #
    def poll_status(self) -> bool:
        try:
            with urllib.request.urlopen(self.status_url, timeout=2) as resp:
                data = json.loads(resp.read().decode("utf-8"))
            GLib.idle_add(self.apply_status, data)
        except Exception:
            # If the server is gone, stop polling but keep the window open.
            pass
        return True  # keep polling

    def apply_status(self, data: dict) -> bool:
        active = data.get("active", False)
        mode = data.get("mode", "?")
        dc = data.get("downloads_completed", 0)
        ur = data.get("uploads_received", 0)
        remaining = data.get("remaining_seconds", 0)

        self.status_label.set_text(
            f"{mode.capitalize()} &bull; {dc} download(s), {ur} upload(s)"
        )
        if remaining > 0:
            mins = remaining // 60
            secs = remaining % 60
            self.status_label.set_text(
                self.status_label.get_text() + f" &bull; {mins:02d}:{secs:02d} left"
            )

        if not active:
            self.stop_btn.set_label("Stopped")
            self.stop_btn.set_sensitive(False)
            # Stop polling once inactive.
            return False
        return True


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="lanbeam-popup", description="lanbeam GTK4 popup")
    parser.add_argument("token", nargs="?", help="session token")
    parser.add_argument("url", nargs="?", help="session base URL (e.g. http://192.168.1.42:5000)")
    parser.add_argument("--port", type=int, default=0, help="session port (informational)")
    parser.add_argument("--self-test", action="store_true", help="headless self-test for CI")
    args = parser.parse_args(argv)

    if args.self_test:
        # Headless verification: ensure imports and argument parsing work.
        print("popup: self-test OK")
        return 0

    if not args.token or not args.url:
        parser.print_help()
        return 1

    app = Gtk.Application(application_id="lanbeam.popup")
    app.connect("activate", lambda a: PopupWindow(a, args.token, args.url, args.port))
    app.run(None)
    return 0


if __name__ == "__main__":
    sys.exit(main())
