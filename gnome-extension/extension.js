import Meta from 'gi://Meta';
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const APP_ID = 'io.github.rclip';
const APP_PATH = '/io/github/rclip';

export default class RclipWatcher extends Extension {
    enable() {
        this._clipboard = St.Clipboard.get_default();
        this._selection = global.display.get_selection();
        this._id = this._selection.connect('owner-changed', (_s, type) => {
            if (type !== Meta.SelectionType.SELECTION_CLIPBOARD) return;

            // Skip password manager copies
            const mimes = this._clipboard.get_mimetypes(St.ClipboardType.CLIPBOARD);
            if (mimes && mimes.includes('x-kde-passwordManagerHint')) return;

            this._clipboard.get_text(St.ClipboardType.CLIPBOARD, (_c, text) => {
                if (!text || !text.trim()) return;
                try {
                    const actions = Gio.DBusActionGroup.get(Gio.DBus.session, APP_ID, APP_PATH);
                    actions.activate_action('add-clip', new GLib.Variant('s', text));
                } catch (e) {
                    // Daemon may be idle or starting
                }
            });
        });
    }

    disable() {
        if (this._id && this._selection) {
            this._selection.disconnect(this._id);
            this._id = null;
        }
    }
}
