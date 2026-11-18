// Unit tests for the pure helper functions in web/app.ts.
// Run with: bun test

import {
  describe,
  expect,
  it,
  beforeEach,
  afterEach,
} from "bun:test";

import {
  formatBytes,
  formatSpeed,
  formatETA,
  extractFilename,
  clamp,
  resolveSessionToken,
  LanbeamUI,
} from "./app.ts";

describe("resolveSessionToken", () => {
  it("prefers injected token over the URL path", () => {
    expect(resolveSessionToken("abc123", "fallback")).toBe("abc123");
  });

  it("falls back to the first path segment and ignores placeholders", () => {
    expect(resolveSessionToken("–", "abc123")).toBe("abc123");
    expect(resolveSessionToken("-", "abc123")).toBe("abc123");
    expect(resolveSessionToken("–", "-")).toBe("");
  });
});

describe("formatBytes", () => {
  it("formats zero bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
  });

  it("formats small bytes", () => {
    expect(formatBytes(512)).toBe("512.00 B");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(1024)).toBe("1.00 KB");
  });

  it("formats megabytes", () => {
    expect(formatBytes(1024 * 1024 * 2.5)).toBe("2.50 MB");
  });

  it("formats gigabytes", () => {
    expect(formatBytes(1024 ** 3 * 3.75)).toBe("3.75 GB");
  });

  it("clamps negative to 0 B", () => {
    expect(formatBytes(-100)).toBe("0 B");
  });
});

describe("formatSpeed", () => {
  it("formats zero bytes per second", () => {
    expect(formatSpeed(0)).toBe("0 B/s");
  });

  it("formats megabytes per second", () => {
    expect(formatSpeed(1024 * 1024 * 5)).toBe("5.00 MB/s");
  });
});

describe("formatETA", () => {
  it("formats seconds under a minute", () => {
    expect(formatETA(1024 * 1024 * 10, 1024 * 1024)).toBe("10s");
  });

  it("formats minutes and seconds", () => {
    expect(formatETA(1024 * 1024 * 90, 1024 * 1024)).toBe("1m 30s");
  });

  it("returns dash when speed is zero", () => {
    expect(formatETA(100, 0)).toBe("–");
  });

  it("returns dash when bytes left is zero", () => {
    expect(formatETA(0, 100)).toBe("–");
  });
});

describe("extractFilename", () => {
  it("returns bare filename", () => {
    expect(extractFilename("photo.jpg")).toBe("photo.jpg");
  });

  it("strips unix path", () => {
    expect(extractFilename("/home/user/pictures/trip.jpg")).toBe("trip.jpg");
  });

  it("strips windows path", () => {
    expect(extractFilename("C:\\Users\\user\\doc.txt")).toBe("doc.txt");
  });

  it("returns empty for trailing slash", () => {
    expect(extractFilename("/some/dir/")).toBe("");
  });
});

describe("clamp", () => {
  it("keeps values within range", () => {
    expect(clamp(5, 0, 10)).toBe(5);
  });

  it("clamps below min", () => {
    expect(clamp(-3, 0, 10)).toBe(0);
  });

  it("clamps above max", () => {
    expect(clamp(15, 0, 10)).toBe(10);
  });

  it("respects boundaries", () => {
    expect(clamp(0, 0, 10)).toBe(0);
    expect(clamp(10, 0, 10)).toBe(10);
  });
});

describe("LanbeamUI initial state seeding", () => {
  const originalWindow = globalThis.window;
  const originalDocument = (globalThis as any).document;
  const originalLocalStorage = globalThis.localStorage;
  const originalFetch = globalThis.fetch;
  const originalUrl = globalThis.URL;

  beforeEach(() => {
    // Create a fresh window mock for each test
    (globalThis as any).window = {
      matchMedia: (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
      setTimeout: globalThis.setTimeout,
      open: () => null,
    };
    // Mock document
    const docStore = new Map<string, string>();
    const makeEl = () => ({
      innerHTML: "",
      textContent: "",
      style: {},
      classList: { add: () => {}, remove: () => {} },
      appendChild: () => {},
      removeChild: () => {},
      click: () => {},
    });
    const elements = new Map<string, any>();
    (globalThis as any).document = {
      documentElement: {
        setAttribute: (name: string, value: string) => { docStore.set(name, value); },
        getAttribute: (name: string) => docStore.get(name) || null,
      },
      getElementById: (id: string) => {
        if (!elements.has(id)) elements.set(id, makeEl());
        return elements.get(id);
      },
      querySelector: () => null,
      querySelectorAll: () => [],
      createElement: () => makeEl(),
      body: { appendChild: () => {}, removeChild: () => {} },
    };
    // Mock localStorage
    const store = new Map<string, string>();
    (globalThis as any).localStorage = {
      getItem: (key: string) => store.get(key) || null,
      setItem: (key: string, value: string) => store.set(key, value),
      removeItem: (key: string) => store.delete(key),
      clear: () => store.clear(),
      get length() { return store.size; },
      key: (index: number) => Array.from(store.keys())[index] || null,
    };
  });

  afterEach(() => {
    (globalThis as any).window = originalWindow;
    (globalThis as any).document = originalDocument;
    (globalThis as any).localStorage = originalLocalStorage;
    (globalThis as any).fetch = originalFetch;
    (globalThis as any).URL = originalUrl;
  });

  it("seeds session from __LANBEAM_INITIAL_STATE__ when present", () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = {
      token: "abc123",
      mode: "send",
      url: "http://192.168.1.5:5000/abc123",
      files: ["photo.jpg", "video.mp4"],
      downloads_completed: 2,
      uploads_received: 1,
      shared_text: "hello world",
      remaining_seconds: 300,
      active: true,
    };

    const ui = new LanbeamUI("abc123", "http://192.168.1.5:5000");
    // Access private state via type assertion for testing
    const state = (ui as any).state as import("./types.ts").AppState;
    const api = (ui as any).api as import("./app.ts").LanbeamApi;

    expect(api.baseUrl).toBe("http://192.168.1.5:5000/abc123");
    expect(state.baseUrl).toBe("http://192.168.1.5:5000");
    expect(state.session).not.toBeNull();
    expect(state.session!.mode).toBe("send");
    expect(state.session!.url).toBe("http://192.168.1.5:5000/abc123");
    expect(state.session!.files).toEqual(["photo.jpg", "video.mp4"]);
    expect(state.files.map((f) => [f.index, f.name])).toEqual([
      [0, "photo.jpg"],
      [1, "video.mp4"],
    ]);
    expect(state.session!.downloads_completed).toBe(2);
    expect(state.session!.uploads_received).toBe(1);
    expect(state.session!.shared_text).toBe("hello world");
    expect(state.session!.remaining_seconds).toBe(300);
    expect(state.session!.active).toBe(true);
  });

  it("downloads files using /<token>/download/<index>/<name>", async () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = {
      token: "tok999",
      mode: "send",
      url: "http://192.168.1.5:5000/tok999",
      files: ["my photo.jpg"],
      active: true,
    };
    const requestedUrls: string[] = [];
    (globalThis as any).fetch = async (url: string) => {
      requestedUrls.push(url);
      return {
        ok: true,
        status: 200,
        blob: async () => new Blob(["ok"]),
      } as any;
    };
    (globalThis as any).URL = {
      createObjectURL: () => "blob:mock",
      revokeObjectURL: () => {},
    };

    const ui = new LanbeamUI("tok999", "http://192.168.1.5:5000");
    const state = (ui as any).state as import("./types.ts").AppState;
    await ui.downloadFile(state.files[0]);

    expect(requestedUrls).toEqual([
      "http://192.168.1.5:5000/tok999/download/0/my%20photo.jpg",
    ]);
    expect(state.files[0].status).toBe("done");
  });

  it("streams nested downloads, reports live speed/ETA, and saves only the basename", async () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = {
      token: "nested-token",
      mode: "send",
      url: "http://192.168.1.5:5000/nested-token",
      files: [],
      active: true,
    };
    const requested: string[] = [];
    const data = [new Uint8Array([1, 2, 3]), new Uint8Array([4, 5, 6])];
    let readIndex = 0;
    (globalThis as any).fetch = async (url: string) => {
      requested.push(url);
      return {
        ok: true,
        status: 200,
        headers: { get: (name: string) => name.toLowerCase() === "content-length" ? "6" : null },
        body: { getReader: () => ({
          read: async () => readIndex < data.length
            ? { done: false, value: data[readIndex++] }
            : { done: true, value: undefined },
          cancel: async () => {},
        }) },
      } as any;
    };
    (globalThis as any).URL = { createObjectURL: () => "blob:stream", revokeObjectURL: () => {} };
    const times = [1000, 2000, 3000];
    const originalNow = Date.now;
    Date.now = () => times.shift() ?? 3000;
    const anchors: any[] = [];
    const doc = (globalThis as any).document;
    const createElement = doc.createElement;
    doc.createElement = (tag: string) => {
      const element = createElement(tag);
      if (tag === "a") anchors.push(element);
      return element;
    };

    try {
      const ui = new LanbeamUI("nested-token", "http://192.168.1.5:5000");
      const state = (ui as any).state as import("./types.ts").AppState;
      const file = { index: 3, name: "nested dir/a #.txt", size: 0, downloaded: false, progress: 0, status: "pending" as const };
      state.files = [file];
      const progressSnapshots: Array<{ speed?: number; eta?: string; progress: number }> = [];
      (ui as any).renderFiles = () => progressSnapshots.push({ speed: file.speed, eta: file.eta, progress: file.progress });

      await ui.downloadFile(file);

      expect(requested).toEqual([
        "http://192.168.1.5:5000/nested-token/download/3/nested%20dir/a%20%23.txt",
      ]);
      expect(file.size).toBe(6);
      expect(file.status).toBe("done");
      expect(progressSnapshots.some((snapshot) => snapshot.progress === 50 && snapshot.speed === 3 && snapshot.eta === "1s")).toBe(true);
      expect(anchors[0].download).toBe("a #.txt");
    } finally {
      Date.now = originalNow;
      doc.createElement = createElement;
    }
  });

  it("reports XHR upload speed/ETA and clears progress state on abort", async () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = { token: "upload-token", mode: "receive", files: [], active: true };
    class FakeXhr {
      upload = { listeners: new Map<string, Function>(), addEventListener: (name: string, fn: Function) => this.upload.listeners.set(name, fn) };
      listeners = new Map<string, Function>();
      status = 200;
      static abortNext = false;
      static throwOnSend = false;
      open() {}
      addEventListener(name: string, fn: Function) { this.listeners.set(name, fn); }
      send() {
        if (FakeXhr.throwOnSend) throw new Error("send failure");
        this.upload.listeners.get("progress")?.({ lengthComputable: true, loaded: 5, total: 10 });
        if (FakeXhr.abortNext) this.listeners.get("abort")?.();
        else this.listeners.get("load")?.();
      }
    }
    (globalThis as any).XMLHttpRequest = FakeXhr;
    const times = [1000, 2000];
    const originalNow = Date.now;
    Date.now = () => times.shift() ?? 2000;
    try {
      const ui = new LanbeamUI("upload-token", "http://192.168.1.5:5000");
      const state = (ui as any).state as import("./types.ts").AppState;
      const progressSnapshots: Array<{ speed?: number; eta?: string; progress: number }> = [];
      (ui as any).renderUploads = () => {
        const upload = state.uploads[state.uploads.length - 1];
        if (upload) progressSnapshots.push({ speed: upload.speed, eta: upload.eta, progress: upload.progress });
      };
      await ui.uploadFileWithProgress({ name: "large.bin", size: 10 } as File);
      expect(progressSnapshots.some((snapshot) => snapshot.progress === 50 && snapshot.speed === 5 && snapshot.eta === "1s")).toBe(true);
      expect(state.uploads[0].status).toBe("done");
      expect(state.uploads[0].speed).toBeUndefined();

      FakeXhr.abortNext = true;
      await expect(ui.uploadFileWithProgress({ name: "cancel.bin", size: 10 } as File)).rejects.toThrow("Upload aborted");
      expect(state.uploads[1].status).toBe("error");
      expect(state.uploads[1].speed).toBeUndefined();
      expect(state.uploads[1].eta).toBeUndefined();

      FakeXhr.abortNext = false;
      FakeXhr.throwOnSend = true;
      await expect(ui.uploadFileWithProgress({ name: "sync-failure.bin", size: 10 } as File)).rejects.toThrow("send failure");
      expect(state.uploads[2].status).toBe("error");
      expect(state.uploads[2].speed).toBeUndefined();
    } finally {
      FakeXhr.abortNext = false;
      FakeXhr.throwOnSend = false;
      Date.now = originalNow;
    }
  });

  it("stopSharing sends POST /<token>/stop and stops polling", async () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = {
      token: "tokstop",
      mode: "send",
      url: "http://192.168.1.5:5000/tokstop",
      files: [],
      active: true,
    };
    const calls: Array<{ url: string; method?: string }> = [];
    (globalThis as any).fetch = async (url: string, init?: RequestInit) => {
      calls.push({ url, method: init?.method });
      return {
        ok: true,
        status: 200,
        text: async () => "Stopping",
      } as any;
    };

    const ui = new LanbeamUI("tokstop", "http://192.168.1.5:5000");
    (ui as any).state.polling = true;
    await ui.stopSharing();

    expect(calls).toEqual([
      { url: "http://192.168.1.5:5000/tokstop/stop", method: "POST" },
    ]);
    expect((ui as any).state.polling).toBe(false);
    expect((ui as any).state.session.active).toBe(false);
  });

  it("falls back to null session when __LANBEAM_INITIAL_STATE__ absent", () => {
    // Ensure no initial state is set
    delete (window as any).__LANBEAM_INITIAL_STATE__;

    const ui = new LanbeamUI("abc123", "http://192.168.1.5:5000");
    const state = (ui as any).state as import("./types.ts").AppState;

    expect(state.session).toBeNull();
  });

  it("handles partial initial state with defaults", () => {
    (window as any).__LANBEAM_INITIAL_STATE__ = {
      mode: "receive",
      url: "http://192.168.1.5:5001/def456",
    };

    const ui = new LanbeamUI("def456", "http://192.168.1.5:5001");
    const state = (ui as any).state as import("./types.ts").AppState;

    expect(state.session).not.toBeNull();
    expect(state.session!.mode).toBe("receive");
    expect(state.session!.url).toBe("http://192.168.1.5:5001/def456");
    expect(state.session!.files).toEqual([]);
    expect(state.session!.downloads_completed).toBe(0);
    expect(state.session!.uploads_received).toBe(0);
    expect(state.session!.shared_text).toBeNull();
    expect(state.session!.remaining_seconds).toBe(0);
    expect(state.session!.active).toBe(true);
  });
});
