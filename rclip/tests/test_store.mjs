import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

import {
    ClipStore,
    isTerminalWmClass,
    parsePngDimensions,
} from '../extension/store.js';

function assert(cond, msg) {
    if (!cond) {
        throw new Error(`Assertion failed: ${msg}`);
    }
}

function assertEq(actual, expected, msg) {
    if (actual !== expected) {
        throw new Error(`Assertion failed (${msg}): expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
}

function makeFakePng(width, height) {
    // 8-byte PNG signature + 4-byte length (13) + 'IHDR' + 4-byte width + 4-byte height + rest
    const bytes = new Uint8Array(32);
    bytes.set([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], 0);
    bytes.set([0x00, 0x00, 0x00, 0x0D], 8);
    bytes.set([0x49, 0x48, 0x44, 0x52], 12);
    bytes[16] = (width >>> 24) & 0xff;
    bytes[17] = (width >>> 16) & 0xff;
    bytes[18] = (width >>> 8) & 0xff;
    bytes[19] = width & 0xff;
    bytes[20] = (height >>> 24) & 0xff;
    bytes[21] = (height >>> 16) & 0xff;
    bytes[22] = (height >>> 8) & 0xff;
    bytes[23] = height & 0xff;
    return new GLib.Bytes(bytes);
}

function removeDirRecursive(path) {
    const dir = Gio.File.new_for_path(path);
    if (!dir.query_exists(null)) return;
    const enumerator = dir.enumerate_children('standard::name,standard::type', Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS, null);
    let info;
    while ((info = enumerator.next_file(null)) !== null) {
        const child = dir.get_child(info.get_name());
        if (info.get_file_type() === Gio.FileType.DIRECTORY) {
            removeDirRecursive(child.get_path());
        } else {
            child.delete(null);
        }
    }
    dir.delete(null);
}

const tmpBase = GLib.dir_make_tmp('rclip-test-XXXXXX');

try {
    // 1. PNG dimension parser & terminal detection
    const fakePng = makeFakePng(1920, 1080);
    const dims = parsePngDimensions(fakePng.get_data());
    assertEq(dims.width, 1920, 'png width');
    assertEq(dims.height, 1080, 'png height');
    assert(isTerminalWmClass('org.gnome.Ptyxis'), 'ptyxis is terminal');
    assert(isTerminalWmClass('Alacritty'), 'alacritty is terminal');
    assert(!isTerminalWmClass('firefox'), 'firefox is not terminal');

    // 2. Text add, dedupe, pin preservation, reload, and pruning
    const store = new ClipStore({ baseDir: tmpBase, maxItems: 3, maxTextBytes: 100, maxImageBytes: 1024 });
    assertEq(store.addText('   \n\t  '), false, 'ignores whitespace');
    assertEq(store.addText('x'.repeat(101)), false, 'rejects oversized text');
    assert(store.addText('first clip'), 'adds first clip');
    assert(store.addText('second clip'), 'adds second clip');

    const firstId = store.items[1].id;
    store.togglePin(firstId);
    assert(store.items[0].pinned, 'pinned item sorts first');

    // Re-adding pinned text preserves pin and moves to top
    store.addText('first clip');
    assertEq(store.items.length, 2, 'dedupes text');
    assert(store.items[0].pinned, 'duplicate preserves pin');

    // Add unpinned clips beyond maxItems=3 -> oldest unpinned pruned, pinned kept
    store.addText('third clip');
    store.addText('fourth clip');
    assertEq(store.items.length, 3, 'respects maxItems');
    assert(store.items.some(i => i.text === 'first clip' && i.pinned), 'pinned survives pruning');

    // 3. Image add, dedupe, thumbnail file creation & cleanup on delete/clear
    const imgAdded = store.addImage(fakePng);
    assert(imgAdded, 'adds image');
    const imgItem = store.items.find(i => i.type === 'image');
    assert(Boolean(imgItem), 'image item exists');
    assertEq(imgItem.width, 1920, 'stored width');
    assertEq(imgItem.height, 1080, 'stored height');
    const imgFile = Gio.File.new_for_path(imgItem.imagePath);
    assert(imgFile.query_exists(null), 'png file written on disk');

    // Reload from disk into fresh instance
    const reloaded = new ClipStore({ baseDir: tmpBase, maxItems: 10, maxTextBytes: 100, maxImageBytes: 1024 });
    assertEq(reloaded.items.length, 3, 'reloaded persisted count');

    // Filter search works across text and image items
    assertEq(reloaded.filterItems('1920x1080').length, 1, 'filters image by dimensions');
    assertEq(reloaded.filterItems('fourth').length, 1, 'filters text by query');

    // Clear unpinned removes image file & unpinned items, keeps pinned
    reloaded.clearUnpinned();
    assertEq(reloaded.items.length, 1, 'only pinned remains after clearUnpinned');
    assert(!imgFile.query_exists(null), 'png file deleted when cleared');

    // Self-copy guard
    reloaded.markSelfCopy('secret-paste');
    assert(reloaded.consumeSelfCopy('secret-paste'), 'consumes self copy once');
    assert(!reloaded.consumeSelfCopy('secret-paste'), 'does not consume twice');

    print('All test_store.mjs checks passed!');
} finally {
    removeDirRecursive(tmpBase);
}
