import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

const TERMINAL_WM_CLASSES = new Set([
    'alacritty',
    'com.mitchellh.ghostty',
    'foot',
    'ghostty',
    'gnome-terminal',
    'gnome-terminal-server',
    'kitty',
    'konsole',
    'org.gnome.console',
    'org.gnome.ptyxis',
    'org.gnome.terminal',
    'org.wezfurlong.wezterm',
    'ptyxis',
    'terminator',
    'tilix',
    'urxvt',
    'wezterm',
    'xfce4-terminal',
    'xterm',
]);

export function isTerminalWmClass(wmClass) {
    if (!wmClass) return false;
    const normalized = wmClass.trim().toLowerCase();
    if (TERMINAL_WM_CLASSES.has(normalized)) return true;
    const shortName = normalized.split('.').pop();
    return TERMINAL_WM_CLASSES.has(shortName);
}

export function parsePngDimensions(bytes) {
    if (!bytes || bytes.length < 24) return { width: 0, height: 0 };
    const sig = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    for (let i = 0; i < 8; i++) {
        if (bytes[i] !== sig[i]) return { width: 0, height: 0 };
    }
    // Check 'IHDR' chunk tag at offset 12..16
    if (bytes[12] !== 0x49 || bytes[13] !== 0x48 || bytes[14] !== 0x44 || bytes[15] !== 0x52) {
        return { width: 0, height: 0 };
    }
    const width = ((bytes[16] << 24) | (bytes[17] << 16) | (bytes[18] << 8) | bytes[19]) >>> 0;
    const height = ((bytes[20] << 24) | (bytes[21] << 16) | (bytes[22] << 8) | bytes[23]) >>> 0;
    return { width, height };
}

function setMode(path, mode) {
    try {
        const file = Gio.File.new_for_path(path);
        file.set_attribute_uint32('unix::mode', mode, Gio.FileQueryInfoFlags.NONE, null);
    } catch (_e) {
        // Best-effort permission hardening
    }
}

function ensureSecureDir(path) {
    GLib.mkdir_with_parents(path, 0o700);
    setMode(path, 0o700);
}

export class ClipStore {
    constructor(options = {}) {
        const dataHome = GLib.get_user_data_dir();
        this.baseDir = options.baseDir || GLib.build_filenamev([dataHome, 'rclip']);
        this.imagesDir = GLib.build_filenamev([this.baseDir, 'images']);
        this.historyPath = GLib.build_filenamev([this.baseDir, 'history.json']);
        this.maxItems = options.maxItems ?? 100;
        this.maxTextBytes = options.maxTextBytes ?? 1_000_000;
        this.maxImageBytes = options.maxImageBytes ?? 10_000_000;
        this.items = [];
        this._selfCopyToken = null;
        this._selfCopyExpiresAt = 0;

        this.load();
    }

    load() {
        ensureSecureDir(this.baseDir);
        ensureSecureDir(this.imagesDir);
        const file = Gio.File.new_for_path(this.historyPath);
        if (!file.query_exists(null)) {
            this.items = [];
            return;
        }
        try {
            const [ok, raw] = file.load_contents(null);
            if (!ok) {
                this.items = [];
                return;
            }
            const parsed = JSON.parse(new TextDecoder().decode(raw));
            const rawItems = Array.isArray(parsed?.items) ? parsed.items : [];
            this.items = rawItems.map(item => this._normalizeLoadedItem(item)).filter(Boolean);
            this._sortItems();
        } catch (_e) {
            this.items = [];
        }
    }

    _normalizeLoadedItem(item) {
        if (!item || typeof item !== 'object') return null;
        if (item.type === 'image' && item.imagePath) {
            const imgFile = Gio.File.new_for_path(item.imagePath);
            if (!imgFile.query_exists(null)) return null;
            return {
                id: String(item.id),
                type: 'image',
                hash: String(item.hash || item.id),
                imagePath: String(item.imagePath),
                width: Number(item.width || 0),
                height: Number(item.height || 0),
                byteSize: Number(item.byteSize || 0),
                pinned: Boolean(item.pinned),
                timestamp: Number(item.timestamp || 0),
            };
        }
        if (typeof item.text === 'string' && item.text.trim().length > 0) {
            const id = item.id !== undefined ? String(item.id) : this._sha256Text(item.text).slice(0, 16);
            return {
                id,
                type: 'text',
                hash: String(item.hash || this._sha256Text(item.text)),
                text: item.text,
                pinned: Boolean(item.pinned),
                timestamp: Number(item.timestamp || 0),
            };
        }
        return null;
    }

    save() {
        ensureSecureDir(this.baseDir);
        const payload = JSON.stringify({ version: 2, items: this.items }, null, 2);
        const tmpPath = `${this.historyPath}.tmp`;
        const bytes = new TextEncoder().encode(payload);
        GLib.file_set_contents(tmpPath, bytes);
        setMode(tmpPath, 0o600);
        const tmpFile = Gio.File.new_for_path(tmpPath);
        const destFile = Gio.File.new_for_path(this.historyPath);
        tmpFile.move(destFile, Gio.FileCopyFlags.OVERWRITE, null, null);
        setMode(this.historyPath, 0o600);
    }

    _sha256Bytes(glibBytes) {
        return GLib.compute_checksum_for_bytes(GLib.ChecksumType.SHA256, glibBytes);
    }

    _sha256Text(text) {
        const encoded = new TextEncoder().encode(text);
        return this._sha256Bytes(new GLib.Bytes(encoded));
    }

    _sortItems() {
        this.items.sort((a, b) => {
            if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
            return b.timestamp - a.timestamp;
        });
    }

    _pruneOverflow() {
        while (this.items.length > this.maxItems) {
            let removeIdx = -1;
            for (let i = this.items.length - 1; i >= 0; i--) {
                if (!this.items[i].pinned) {
                    removeIdx = i;
                    break;
                }
            }
            if (removeIdx === -1) break;
            const [removed] = this.items.splice(removeIdx, 1);
            this._removeImageAsset(removed);
        }
    }

    _removeImageAsset(item) {
        if (item?.type !== 'image' || !item.imagePath) return;
        try {
            const file = Gio.File.new_for_path(item.imagePath);
            if (file.query_exists(null)) file.delete(null);
        } catch (_e) {
            // Ignore cleanup errors
        }
    }

    addText(text) {
        if (typeof text !== 'string' || text.trim().length === 0) return false;
        const encoded = new TextEncoder().encode(text);
        if (encoded.length > this.maxTextBytes) return false;

        const hash = this._sha256Bytes(new GLib.Bytes(encoded));
        const now = Math.floor(Date.now());
        const existingIdx = this.items.findIndex(i => i.type === 'text' && i.hash === hash);
        if (existingIdx !== -1) {
            const [existing] = this.items.splice(existingIdx, 1);
            existing.timestamp = now;
            this.items.unshift(existing);
        } else {
            this.items.unshift({
                id: `${now}-${hash.slice(0, 8)}`,
                type: 'text',
                hash,
                text,
                pinned: false,
                timestamp: now,
            });
        }
        this._sortItems();
        this._pruneOverflow();
        this.save();
        return true;
    }

    addImage(glibBytes) {
        if (!glibBytes) return false;
        const byteSize = glibBytes.get_size();
        if (byteSize === 0 || byteSize > this.maxImageBytes) return false;

        const hash = this._sha256Bytes(glibBytes);
        const now = Math.floor(Date.now());
        const existingIdx = this.items.findIndex(i => i.type === 'image' && i.hash === hash);
        if (existingIdx !== -1) {
            const [existing] = this.items.splice(existingIdx, 1);
            existing.timestamp = now;
            this.items.unshift(existing);
        } else {
            ensureSecureDir(this.imagesDir);
            const id = `${now}-${hash.slice(0, 8)}`;
            const imagePath = GLib.build_filenamev([this.imagesDir, `${id}.png`]);
            const raw = glibBytes.get_data();
            const dims = parsePngDimensions(raw);
            GLib.file_set_contents(imagePath, raw);
            setMode(imagePath, 0o600);
            this.items.unshift({
                id,
                type: 'image',
                hash,
                imagePath,
                width: dims.width,
                height: dims.height,
                byteSize,
                pinned: false,
                timestamp: now,
            });
        }
        this._sortItems();
        this._pruneOverflow();
        this.save();
        return true;
    }

    togglePin(id) {
        const item = this.items.find(i => i.id === id);
        if (!item) return false;
        item.pinned = !item.pinned;
        this._sortItems();
        this.save();
        return true;
    }

    deleteItem(id) {
        const idx = this.items.findIndex(i => i.id === id);
        if (idx === -1) return false;
        const [removed] = this.items.splice(idx, 1);
        this._removeImageAsset(removed);
        this.save();
        return true;
    }

    clearUnpinned() {
        const kept = [];
        for (const item of this.items) {
            if (item.pinned) {
                kept.push(item);
            } else {
                this._removeImageAsset(item);
            }
        }
        this.items = kept;
        this.save();
    }

    filterItems(query) {
        const q = (query || '').trim().toLowerCase();
        if (!q) return [...this.items];
        const terms = q.split(/\s+/).filter(Boolean);
        return this.items.filter(item => {
            const haystack = item.type === 'image'
                ? `image png ${item.width}x${item.height}`.toLowerCase()
                : item.text.toLowerCase();
            return terms.every(t => haystack.includes(t));
        });
    }

    markSelfCopy(token) {
        this._selfCopyToken = token;
        this._selfCopyExpiresAt = Date.now() + 2500;
    }

    consumeSelfCopy(token) {
        if (!this._selfCopyToken || Date.now() > this._selfCopyExpiresAt) {
            this._selfCopyToken = null;
            return false;
        }
        if (this._selfCopyToken === token) {
            this._selfCopyToken = null;
            return true;
        }
        return false;
    }
}
