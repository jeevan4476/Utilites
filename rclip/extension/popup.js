import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GObject from 'gi://GObject';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const POPUP_WIDTH = 372;
const POPUP_HEIGHT = 464;
const EDGE_MARGIN = 12;

function formatRelativeTime(timestampMs) {
    if (!timestampMs) return '';
    const diffSec = Math.max(0, Math.floor((Date.now() - timestampMs) / 1000));
    if (diffSec < 60) return 'Just now';
    const diffMin = Math.floor(diffSec / 60);
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) return `${diffHr}h ago`;
    const diffDays = Math.floor(diffHr / 24);
    return `${diffDays}d ago`;
}

function formatBytes(bytes) {
    if (!bytes) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatTextPreview(text) {
    const lines = text
        .trim()
        .split(/\r?\n/)
        .map(line => line.trim())
        .filter(Boolean);
    const preview = lines.slice(0, 3).join('\n');
    return preview.length > 220 ? `${preview.slice(0, 217)}...` : preview;
}

export const RclipPopup = GObject.registerClass(
    class RclipPopup extends St.BoxLayout {
        _init(store, onPasteItem) {
            super._init({
                style_class: 'rclip-popup',
                orientation: Clutter.Orientation.VERTICAL,
                reactive: true,
                visible: false,
            });

            this._store = store;
            this._onPasteItem = onPasteItem;
            this._modalGrab = null;
            this._filtered = [];
            this._cards = [];
            this._selectedIdx = 0;

            this._keyController = new Clutter.KeyController();
            this._keyController.connect('key-press', () => this._onKeyPress());

            this._clickOutsideGesture = new Clutter.ClickGesture({
                recognize_on_press: true,
            });
            this._clickOutsideGesture.connect('may-recognize', () => {
                const event = this._clickOutsideGesture.get_point_event(0);
                const targetActor = event ? global.stage.get_event_actor(event) : null;
                return !this.contains(targetActor);
            });
            this._clickOutsideGesture.connect('recognize', () => {
                this.close();
            });

            this._buildUi();
        }

        _buildUi() {
            const header = new St.BoxLayout({ style_class: 'rclip-header', x_expand: true });
            const title = new St.Label({
                text: 'Clipboard',
                style_class: 'rclip-title',
                x_expand: true,
                y_align: Clutter.ActorAlign.CENTER,
            });
            const clearBtn = new St.Button({
                label: 'Clear all',
                style_class: 'rclip-clear-btn',
                can_focus: true,
            });
            clearBtn.connect('clicked', () => {
                this._store.clearUnpinned();
                this.refresh();
            });
            header.add_child(title);
            header.add_child(clearBtn);
            this.add_child(header);

            this._searchEntry = new St.Entry({
                style_class: 'rclip-search',
                hint_text: 'Search clipboard...',
                can_focus: true,
                x_expand: true,
            });
            this._searchEntry.clutter_text.connect('text-changed', () => {
                this._selectedIdx = 0;
                this.refresh();
            });
            this.add_child(this._searchEntry);

            this._scrollView = new St.ScrollView({
                style_class: 'rclip-scroll',
                x_expand: true,
                y_expand: true,
                hscrollbar_policy: St.PolicyType.NEVER,
                vscrollbar_policy: St.PolicyType.AUTOMATIC,
                overlay_scrollbars: true,
            });
            this._listBox = new St.BoxLayout({
                style_class: 'rclip-list',
                orientation: Clutter.Orientation.VERTICAL,
                x_expand: true,
            });
            this._scrollView.set_child(this._listBox);
            this.add_child(this._scrollView);

            const footer = new St.Label({
                text: '↵ Paste  •  Click Paste  •  Ctrl+P Pin  •  Ctrl+D Delete  •  Esc Close',
                style_class: 'rclip-footer',
                x_align: Clutter.ActorAlign.CENTER,
            });
            this.add_child(footer);
        }

        _positionAtCursor() {
            const [mx, my] = global.get_pointer();
            let monitorIdx = Main.layoutManager.primaryIndex;
            const monitors = Main.layoutManager.monitors || [];
            for (let i = 0; i < monitors.length; i++) {
                const m = monitors[i];
                if (mx >= m.x && mx < m.x + m.width && my >= m.y && my < m.y + m.height) {
                    monitorIdx = i;
                    break;
                }
            }
            const workArea = Main.layoutManager.getWorkAreaForMonitor(monitorIdx);
            const minX = workArea.x + EDGE_MARGIN;
            const maxX = Math.max(minX, workArea.x + workArea.width - POPUP_WIDTH - EDGE_MARGIN);
            const minY = workArea.y + EDGE_MARGIN;
            const maxY = Math.max(minY, workArea.y + workArea.height - POPUP_HEIGHT - EDGE_MARGIN);

            const x = Math.min(Math.max(mx + 8, minX), maxX);
            const y = Math.min(Math.max(my + 8, minY), maxY);
            this.set_position(x, y);
        }

        open() {
            if (this.visible) return;
            if (!this.get_parent()) {
                Main.layoutManager.addTopChrome(this, { affectsStruts: false });
            }
            this._searchEntry.set_text('');
            this._selectedIdx = 0;
            this.refresh();
            this._positionAtCursor();
            this.show();

            this._modalGrab = Main.pushModal(this, { actionMode: Shell.ActionMode.POPUP });
            this.add_action_full(
                'rclip-key-capture',
                Clutter.EventPhase.CAPTURE,
                this._keyController
            );
            this.add_action_full(
                'rclip-click-outside',
                Clutter.EventPhase.CAPTURE,
                this._clickOutsideGesture
            );
            this._searchEntry.grab_key_focus();
        }

        close() {
            if (!this.visible) return;
            this.remove_action(this._keyController);
            this.remove_action(this._clickOutsideGesture);
            if (this._modalGrab) {
                Main.popModal(this._modalGrab);
                this._modalGrab = null;
            }
            this.hide();
        }

        _onKeyPress() {
            const [, key] = this._keyController.get_key();
            const [, pressed, latched, locked] = this._keyController.get_state();
            const state = pressed | latched | locked;
            const ctrl = Boolean(state & Clutter.ModifierType.CONTROL_MASK);

            if (key === Clutter.KEY_Escape) {
                this.close();
                return Clutter.EVENT_STOP;
            }
            if (key === Clutter.KEY_Down) {
                this._moveSelection(1);
                return Clutter.EVENT_STOP;
            }
            if (key === Clutter.KEY_Up) {
                this._moveSelection(-1);
                return Clutter.EVENT_STOP;
            }
            if (
                key === Clutter.KEY_Return ||
                key === Clutter.KEY_KP_Enter ||
                key === Clutter.KEY_ISO_Enter
            ) {
                this._activateSelected();
                return Clutter.EVENT_STOP;
            }
            if (ctrl && (key === Clutter.KEY_p || key === Clutter.KEY_P)) {
                this._togglePinSelected();
                return Clutter.EVENT_STOP;
            }
            if (
                (ctrl && (key === Clutter.KEY_d || key === Clutter.KEY_D)) ||
                (key === Clutter.KEY_Delete && this._searchEntry.get_text().length === 0)
            ) {
                this._deleteSelected();
                return Clutter.EVENT_STOP;
            }

            return Clutter.EVENT_PROPAGATE;
        }

        _moveSelection(delta) {
            if (this._cards.length === 0) return;
            const next = Math.max(0, Math.min(this._cards.length - 1, this._selectedIdx + delta));
            if (next === this._selectedIdx) return;
            this._cards[this._selectedIdx]?.remove_style_class_name('rclip-card-selected');
            this._selectedIdx = next;
            const activeCard = this._cards[this._selectedIdx];
            activeCard.add_style_class_name('rclip-card-selected');
            this._scrollCardIntoView(activeCard);
        }

        _scrollCardIntoView(card) {
            const adj = this._scrollView.get_vadjustment();
            if (!adj) return;
            const box = card.get_allocation_box();
            const viewTop = adj.value;
            const viewBottom = viewTop + adj.page_size;
            if (box.y1 < viewTop) {
                adj.set_value(box.y1);
            } else if (box.y2 > viewBottom) {
                adj.set_value(box.y2 - adj.page_size);
            }
        }

        _activateSelected() {
            const item = this._filtered[this._selectedIdx];
            if (!item) return;
            this._onPasteItem(item);
        }

        _togglePinSelected() {
            const item = this._filtered[this._selectedIdx];
            if (!item) return;
            this._store.togglePin(item.id);
            this.refresh();
        }

        _deleteSelected() {
            const item = this._filtered[this._selectedIdx];
            if (!item) return;
            this._store.deleteItem(item.id);
            this.refresh();
        }

        refresh() {
            this._listBox.destroy_all_children();
            this._cards = [];

            const query = this._searchEntry.get_text();
            this._filtered = this._store.filterItems(query);

            if (this._filtered.length === 0) {
                const empty = new St.Label({
                    text: query.trim() ? 'No matching clips' : 'Clipboard history is empty',
                    style_class: 'rclip-empty',
                    x_expand: true,
                });
                this._listBox.add_child(empty);
                this._selectedIdx = 0;
                return;
            }

            if (this._selectedIdx >= this._filtered.length) {
                this._selectedIdx = this._filtered.length - 1;
            }

            for (let i = 0; i < this._filtered.length; i++) {
                const item = this._filtered[i];
                const card = this._buildCard(item, i);
                if (i === this._selectedIdx) {
                    card.add_style_class_name('rclip-card-selected');
                }
                this._cards.push(card);
                this._listBox.add_child(card);
            }
        }

        _buildCard(item, index) {
            const card = new St.BoxLayout({
                style_class: 'rclip-card',
                reactive: true,
                track_hover: true,
                x_expand: true,
            });

            const body = new St.BoxLayout({
                style_class: 'rclip-card-body',
                orientation: Clutter.Orientation.VERTICAL,
                x_expand: true,
            });

            if (item.type === 'image') {
                const uri = Gio.File.new_for_path(item.imagePath).get_uri().replace(/'/g, '%27');
                const thumb = new St.Bin({
                    style_class: 'rclip-thumb',
                    style: `background-image: url('${uri}'); background-size: contain; background-repeat: no-repeat; background-position: center;`,
                    x_align: Clutter.ActorAlign.START,
                });
                body.add_child(thumb);

                const dimsText = item.width > 0 ? `${item.width}×${item.height} PNG` : 'PNG Image';
                const meta = new St.Label({
                    text: `${dimsText} • ${formatBytes(item.byteSize)} • ${formatRelativeTime(item.timestamp)}`,
                    style_class: 'rclip-card-meta',
                });
                body.add_child(meta);
            } else {
                const textLabel = new St.Label({
                    text: formatTextPreview(item.text),
                    style_class: 'rclip-card-text',
                    x_expand: true,
                });
                textLabel.clutter_text.set_line_wrap(true);
                textLabel.clutter_text.set_line_wrap_mode(Pango.WrapMode.WORD_CHAR);
                textLabel.clutter_text.set_ellipsize(Pango.EllipsizeMode.END);
                body.add_child(textLabel);

                const meta = new St.Label({
                    text: formatRelativeTime(item.timestamp),
                    style_class: 'rclip-card-meta',
                });
                body.add_child(meta);
            }

            const actions = new St.BoxLayout({
                style_class: 'rclip-card-actions',
                y_align: Clutter.ActorAlign.CENTER,
            });

            const pinBtn = new St.Button({
                label: item.pinned ? '★' : '☆',
                style_class: item.pinned ? 'rclip-icon-btn rclip-pin-active' : 'rclip-icon-btn',
                reactive: true,
            });
            pinBtn.connect('clicked', () => {
                this._store.togglePin(item.id);
                this.refresh();
                return Clutter.EVENT_STOP;
            });

            const delBtn = new St.Button({
                label: '✕',
                style_class: 'rclip-icon-btn rclip-del-btn',
                reactive: true,
            });
            delBtn.connect('clicked', () => {
                this._store.deleteItem(item.id);
                this.refresh();
                return Clutter.EVENT_STOP;
            });

            actions.add_child(pinBtn);
            actions.add_child(delBtn);

            card.add_child(body);
            card.add_child(actions);

            const cardClick = new Clutter.ClickGesture();
            cardClick.connect('may-recognize', () => {
                if (cardClick.get_button() !== 1) return false;
                const event = cardClick.get_point_event(0);
                const targetActor = event ? global.stage.get_event_actor(event) : null;
                return !actions.contains(targetActor);
            });
            cardClick.connect('recognize', () => {
                this._selectedIdx = index;
                this._onPasteItem(item);
            });
            card.add_action(cardClick);

            return card;
        }

        destroy() {
            this.close();
            if (this.get_parent()) {
                this.get_parent().remove_child(this);
            }
            super.destroy();
        }
    }
);
