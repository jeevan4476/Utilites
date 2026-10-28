import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import { RclipPopup } from './popup.js';
import { ClipStore, isTerminalWmClass } from './store.js';

const DBUS_IFACE = `
<node>
  <interface name="io.github.rclip">
    <method name="Toggle"/>
    <method name="Show"/>
    <method name="Hide"/>
    <method name="Clear"/>
  </interface>
</node>`;

const BUS_NAME = 'io.github.rclip';
const OBJ_PATH = '/io/github/rclip';
const KEYBINDING_NAME = 'toggle-popup';

export default class RclipExtension extends Extension {
    enable() {
        this._store = new ClipStore();
        this._clipboard = St.Clipboard.get_default();
        this._selection = global.display.get_selection();
        this._popup = new RclipPopup(this._store, item => this._pasteItem(item));
        this._previousWindow = null;
        this._pasteTimeoutId = 0;

        const seat = Clutter.get_default_backend().get_default_seat();
        this._virtualKeyboard = seat.create_virtual_device(
            Clutter.InputDeviceType.KEYBOARD_DEVICE
        );

        this._selectionId = this._selection.connect('owner-changed', (_sel, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD) {
                this._captureClipboard();
            }
        });

        this._settings = this.getSettings();
        Main.wm.addKeybinding(
            KEYBINDING_NAME,
            this._settings,
            Meta.KeyBindingFlags.NONE,
            Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP,
            () => this.Toggle()
        );

        this._dbusImpl = Gio.DBusExportedObject.wrapJSObject(DBUS_IFACE, this);
        this._dbusImpl.export(Gio.DBus.session, OBJ_PATH);
        this._busOwnerId = Gio.bus_own_name_on_connection(
            Gio.DBus.session,
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null
        );
    }

    disable() {
        if (this._pasteTimeoutId) {
            GLib.Source.remove(this._pasteTimeoutId);
            this._pasteTimeoutId = 0;
        }
        Main.wm.removeKeybinding(KEYBINDING_NAME);

        if (this._selectionId && this._selection) {
            this._selection.disconnect(this._selectionId);
            this._selectionId = 0;
        }
        if (this._busOwnerId) {
            Gio.bus_unown_name(this._busOwnerId);
            this._busOwnerId = 0;
        }
        if (this._dbusImpl) {
            this._dbusImpl.unexport();
            this._dbusImpl = null;
        }
        if (this._popup) {
            this._popup.destroy();
            this._popup = null;
        }
        this._virtualKeyboard = null;
        this._previousWindow = null;
        this._settings = null;
        this._store = null;
    }

    Toggle() {
        if (this._popup.visible) {
            this.Hide();
        } else {
            this.Show();
        }
    }

    Show() {
        if (this._popup.visible) return;
        this._previousWindow = global.display.focus_window;
        this._popup.open();
    }

    Hide() {
        this._popup?.close();
    }

    Clear() {
        this._store?.clearUnpinned();
        if (this._popup?.visible) {
            this._popup.refresh();
        }
    }

    _captureClipboard() {
        const mimes = this._clipboard.get_mimetypes(St.ClipboardType.CLIPBOARD) || [];
        if (mimes.includes('x-kde-passwordManagerHint')) return;

        const hasText = mimes.some(
            m => m.startsWith('text/plain') || m === 'UTF8_STRING' || m === 'STRING' || m === 'TEXT'
        );
        const hasPng = mimes.includes('image/png');

        if (hasPng && !hasText) {
            this._clipboard.get_content(St.ClipboardType.CLIPBOARD, 'image/png', (_c, bytes) => {
                if (!bytes || !this._store) return;
                const hash = GLib.compute_checksum_for_bytes(GLib.ChecksumType.SHA256, bytes);
                if (this._store.consumeSelfCopy(hash)) return;
                if (this._store.addImage(bytes) && this._popup?.visible) {
                    this._popup.refresh();
                }
            });
            return;
        }

        if (hasText || mimes.length === 0) {
            this._clipboard.get_text(St.ClipboardType.CLIPBOARD, (_c, text) => {
                if (!text || !this._store) return;
                const hash = GLib.compute_checksum_for_bytes(
                    GLib.ChecksumType.SHA256,
                    new GLib.Bytes(new TextEncoder().encode(text))
                );
                if (this._store.consumeSelfCopy(hash)) return;
                if (this._store.addText(text) && this._popup?.visible) {
                    this._popup.refresh();
                }
            });
        }
    }

    _pasteItem(item) {
        const targetWin = this._previousWindow;
        this._previousWindow = null;
        this._popup.close();

        this._store.markSelfCopy(item.hash);

        if (item.type === 'image') {
            try {
                const file = Gio.File.new_for_path(item.imagePath);
                const [ok, raw] = file.load_contents(null);
                if (!ok) return;
                this._clipboard.set_content(
                    St.ClipboardType.CLIPBOARD,
                    'image/png',
                    new GLib.Bytes(raw)
                );
            } catch (_e) {
                return;
            }
        } else {
            this._clipboard.set_text(St.ClipboardType.CLIPBOARD, item.text);
        }

        if (targetWin) {
            try {
                targetWin.activate(global.get_current_time());
            } catch (_e) {
                // Window may have closed while popup was open
            }
        }

        if (this._pasteTimeoutId) {
            GLib.Source.remove(this._pasteTimeoutId);
        }
        const useTerminalShortcut = isTerminalWmClass(targetWin?.get_wm_class?.());
        this._pasteTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 60, () => {
            this._pasteTimeoutId = 0;
            this._sendPasteKeystroke(useTerminalShortcut);
            return GLib.SOURCE_REMOVE;
        });
    }

    _sendPasteKeystroke(useShift) {
        if (!this._virtualKeyboard) return;
        const pressTime = GLib.get_monotonic_time();
        const releaseTime = pressTime + 5000;
        const vKey = useShift ? Clutter.KEY_V : Clutter.KEY_v;

        this._virtualKeyboard.notify_keyval(
            pressTime,
            Clutter.KEY_Control_L,
            Clutter.KeyState.PRESSED
        );
        if (useShift) {
            this._virtualKeyboard.notify_keyval(
                pressTime,
                Clutter.KEY_Shift_L,
                Clutter.KeyState.PRESSED
            );
        }
        this._virtualKeyboard.notify_keyval(pressTime, vKey, Clutter.KeyState.PRESSED);

        this._virtualKeyboard.notify_keyval(releaseTime, vKey, Clutter.KeyState.RELEASED);
        if (useShift) {
            this._virtualKeyboard.notify_keyval(
                releaseTime,
                Clutter.KEY_Shift_L,
                Clutter.KeyState.RELEASED
            );
        }
        this._virtualKeyboard.notify_keyval(
            releaseTime,
            Clutter.KEY_Control_L,
            Clutter.KeyState.RELEASED
        );
    }
}
