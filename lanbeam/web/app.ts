// lanbeam mobile web frontend — application logic.
//
// Pure helper functions (byte size, speed, ETA, filename extraction) are
// unit-tested in web/app.test.ts. The API client talks to the Rust backend
// at GET /<token>/status, POST /<token>/upload?name=<name>, and
// POST /<token>/text. The UI renders a responsive dark/light mobile page
// with file downloads, an upload queue with progress bars, and two-way
// clipboard/link sharing.

import type {
  AppState,
  FileInfo,
  LanbeamMode,
  SessionState,
  UploadInfo,
} from "./types.ts";

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"];

/** Format a byte count into a human-readable string (e.g. "1.23 MB"). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "0 B";
  if (bytes === 0) return "0 B";
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const unit = SIZE_UNITS[Math.min(i, SIZE_UNITS.length - 1)];
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(2)} ${unit}`;
}

/** Format a transfer speed in bytes per second (e.g. "4.56 MB/s"). */
export function formatSpeed(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond < 0)
    return "0 B/s";
  if (bytesPerSecond === 0) return "0 B/s";
  const i = Math.floor(Math.log(bytesPerSecond) / Math.log(1024));
  const unit = SIZE_UNITS[Math.min(i, SIZE_UNITS.length - 1)];
  const value = bytesPerSecond / Math.pow(1024, i);
  return `${value.toFixed(2)} ${unit}/s`;
}

/** Estimate remaining seconds given bytes left and current speed (B/s). */
export function formatETA(bytesLeft: number, bytesPerSecond: number): string {
  if (!Number.isFinite(bytesLeft) || bytesLeft <= 0) return "–";
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "–";
  const seconds = Math.ceil(bytesLeft / bytesPerSecond);
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}m ${s}s`;
}

/** Extract the base filename from a path, stripping directories. */
export function extractFilename(path: string): string {
  const lastSlash = Math.max(
    path.lastIndexOf("/"),
    path.lastIndexOf("\\")
  );
  return lastSlash === -1 ? path : path.slice(lastSlash + 1);
}

/** Clamp a value between min and max. */
export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function resolveSessionToken(embeddedToken?: string, pathToken?: string): string {
  for (const candidate of [embeddedToken, pathToken]) {
    const token = candidate?.trim() ?? "";
    if (token && token !== "–" && token !== "-") return token;
  }
  return "";
}

// ---------------------------------------------------------------------------
// API client
// ---------------------------------------------------------------------------

export class LanbeamApi {
  constructor(public baseUrl: string) {}

  private async request(
    path: string,
    opts: RequestInit = {}
  ): Promise<Response> {
    const url = `${this.baseUrl}${path}`;
    const res = await fetch(url, {
      headers: { "Content-Type": "application/json", ...opts.headers },
      ...opts,
    });
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(`API ${res.status} ${res.statusText}: ${body}`);
    }
    return res;
  }

  /** Fetch session state from GET /<token>/status. */
  async getStatus(): Promise<SessionState> {
    const res = await this.request("/status");
    return (await res.json()) as SessionState;
  }

  /** Upload a file via POST /<token>/upload?name=<name>. */
  async uploadFile(name: string, data: BodyInit): Promise<void> {
    await this.request(`/upload?name=${encodeURIComponent(name)}`, {
      method: "POST",
      body: data,
    });
  }

  /** Send text/link via POST /<token>/text. */
  async sendText(text: string): Promise<void> {
    await this.request("/text", {
      method: "POST",
      body: JSON.stringify({ text }),
    });
  }

  /** Stop the active sharing session via POST /<token>/stop. */
  async stopSession(): Promise<void> {
    await this.request("/stop", {
      method: "POST",
    });
  }
}

// ---------------------------------------------------------------------------
// UI controller
// ---------------------------------------------------------------------------

export class LanbeamUI {
  private state: AppState;
  private api: LanbeamApi;
  private pollTimer: number | null = null;

  // DOM elements (cached after first render).
  private els: Record<string, HTMLElement | null> = {};

  constructor(token: string, baseUrl: string) {
    // Seed initial state from server-injected global if present
    const initialState = (window as any).__LANBEAM_INITIAL_STATE__ as
      | { token?: string; mode?: string; url?: string; files?: string[]; downloads_completed?: number; uploads_received?: number; shared_text?: string | null; remaining_seconds?: number; active?: boolean }
      | undefined;
    const embeddedToken = initialState?.token?.trim();
    const pathToken = (window as any).location?.pathname?.split("/").filter(Boolean)[0] || "";
    const resolvedToken = resolveSessionToken(embeddedToken, pathToken) || token;
    const cleanBase = baseUrl.replace(/\/+$/, "");
    const rootBase =
      resolvedToken && cleanBase.endsWith(`/${resolvedToken}`)
        ? cleanBase.slice(0, -(resolvedToken.length + 1))
        : cleanBase;
    const apiBase = resolvedToken ? `${rootBase}/${resolvedToken}` : rootBase;

    this.state = {
      token: resolvedToken,
      baseUrl: rootBase,
      session: initialState
        ? {
            token: resolvedToken || undefined,
            mode: (initialState.mode as any) || "send",
            url: initialState.url || "",
            files: initialState.files || [],
            downloads_completed: initialState.downloads_completed || 0,
            uploads_received: initialState.uploads_received || 0,
            shared_text: initialState.shared_text ?? null,
            remaining_seconds: initialState.remaining_seconds || 0,
            active: initialState.active ?? true,
          }
        : null,
      files: [],
      uploads: [],
      theme: "light",
      polling: false,
    };
    if (this.state.session) {
      this.syncFilesFromSession(this.state.session);
    }
    this.api = new LanbeamApi(apiBase);
    this.initTheme();
  }

  // -- lifecycle -----------------------------------------------------------

  start(): void {
    this.state.polling = true;
    this.render();
    this.poll();
  }

  stop(): void {
    this.state.polling = false;
    if (this.pollTimer !== null) {
      clearTimeout(this.pollTimer);
      this.pollTimer = null;
    }
  }

  private poll(): void {
    if (!this.state.polling) return;
    this.refreshStatus()
      .catch((err) => {
        console.error("lanbeam: poll failed:", err);
        // Keep polling; the server may be restarting.
      })
      .finally(() => {
        if (this.state.polling) {
          this.pollTimer = window.setTimeout(() => this.poll(), 1000);
        }
      });
  }

  private async refreshStatus(): Promise<void> {
    const session = await this.api.getStatus();
    if (session.token) {
      this.state.token = session.token;
    }
    this.state.session = session;
    this.syncFilesFromSession(session);
    this.render();
    if (!session.active) {
      this.stop();
    }
  }

  private syncFilesFromSession(session: SessionState): void {
    // Merge session file list into our local file model, preserving progress.
    const byKey = new Map(
      this.state.files.map((f) => [`${f.index}:${f.name}`, f])
    );
    const next: FileInfo[] = session.files.map((name, index) => {
      const existing = byKey.get(`${index}:${name}`);
      if (existing) return existing;
      return {
        name,
        index,
        size: 0,
        downloaded: false,
        progress: 0,
        status: "pending",
      };
    });
    this.state.files = next;
  }

  // -- theme --------------------------------------------------------------

  private initTheme(): void {
    const saved = localStorage.getItem("lanbeam-theme") as
      | "light"
      | "dark"
      | null;
    if (saved) this.state.theme = saved;
    else if (window.matchMedia("(prefers-color-scheme: dark)").matches)
      this.state.theme = "dark";
    this.applyTheme();
  }

  toggleTheme(): void {
    this.state.theme = this.state.theme === "light" ? "dark" : "light";
    localStorage.setItem("lanbeam-theme", this.state.theme);
    this.applyTheme();
    this.render();
  }

  private applyTheme(): void {
    document.documentElement.setAttribute("data-theme", this.state.theme);
  }

  // -- rendering ----------------------------------------------------------

  private el(id: string): HTMLElement {
    let existing = this.els[id];
    if (!existing) {
      existing = document.getElementById(id);
      this.els[id] = existing!;
    }
    return existing!;
  }

  render(): void {
    const s = this.state.session;
    if (!s) return;

    // Session header.
    this.setText("session-token", this.state.token || s.token || "–");
    this.setText("session-url", s.url);
    this.setText("session-mode", s.mode.charAt(0).toUpperCase() + s.mode.slice(1));
    this.setText(
      "session-active",
      s.active ? "Active" : "Inactive"
    );
    this.setText(
      "session-counters",
      `Downloads: ${s.downloads_completed} · Uploads: ${s.uploads_received}`
    );
    this.setText(
      "session-time",
      s.remaining_seconds > 0
        ? `Time left: ${Math.ceil(s.remaining_seconds / 60)} min`
        : "No time limit"
    );

    // Session QR code.
    const qrWrap = this.el("session-qr-wrap");
    const qrImg = this.el("session-qr") as HTMLImageElement | null;
    const token = this.activeToken();
    if (qrWrap?.classList && qrImg) {
      if (token) {
        const qrSrc = `${this.state.baseUrl}/${token}/qr.svg`;
        if (qrImg.src !== qrSrc) {
          qrImg.src = qrSrc;
        }
        qrWrap.classList.remove("hidden");
      } else {
        qrWrap.classList.add("hidden");
      }
    }

    // Shared text banner.
    const sharedTextEl = this.el("shared-text");
    const sharedTextBody = this.el("shared-text-body");
    if (s.shared_text) {
      sharedTextEl.classList.remove("hidden");
      sharedTextBody.textContent = s.shared_text;
    } else {
      sharedTextEl.classList.add("hidden");
    }

    // Files list.
    this.renderFiles();
    this.renderUploads();
  }

  private renderFiles(): void {
    const container = this.el("file-list");
    container.innerHTML = "";
    if (this.state.files.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "No files shared yet.";
      container.appendChild(empty);
      return;
    }
    for (const file of this.state.files) {
      container.appendChild(this.buildFileRow(file));
    }
  }

  private buildFileRow(f: FileInfo): HTMLElement {
    const row = document.createElement("div");
    row.className = `file-row status-${f.status}`;

    const nameCell = document.createElement("div");
    nameCell.className = "file-name";
    nameCell.textContent = f.name;

    const sizeCell = document.createElement("div");
    sizeCell.className = "file-size";
    let sizeText = f.size > 0 ? formatBytes(f.size) : "–";
    if (f.speed !== undefined && f.speed > 0) {
      sizeText += ` · ${formatSpeed(f.speed)}`;
      if (f.eta) sizeText += ` · ETA ${f.eta}`;
    }
    sizeCell.textContent = sizeText;

    const progCell = document.createElement("div");
    progCell.className = "file-progress";
    const bar = document.createElement("div");
    bar.className = "progress-bar";
    const fill = document.createElement("div");
    fill.className = "progress-fill";
    fill.style.width = `${clamp(f.progress, 0, 100)}%`;
    bar.appendChild(fill);
    progCell.appendChild(bar);

    const actCell = document.createElement("div");
    actCell.className = "file-actions";

    if (f.status === "pending" || f.status === "error") {
      const btn = document.createElement("button");
      btn.className = "btn btn-primary btn-sm";
      btn.textContent = f.status === "error" ? "Retry" : "Download";
      btn.onclick = () => this.downloadFile(f);
      actCell.appendChild(btn);
    } else if (f.status === "done") {
      const btn = document.createElement("button");
      btn.className = "btn btn-outline btn-sm";
      btn.textContent = "Open";
      btn.onclick = () => this.openFile(f);
      actCell.appendChild(btn);
    }

    row.appendChild(nameCell);
    row.appendChild(sizeCell);
    row.appendChild(progCell);
    row.appendChild(actCell);
    return row;
  }

  private renderUploads(): void {
    const container = this.el("upload-list");
    container.innerHTML = "";
    if (this.state.uploads.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "No uploads queued.";
      container.appendChild(empty);
      return;
    }
    for (const up of this.state.uploads) {
      container.appendChild(this.buildUploadRow(up));
    }
  }

  private buildUploadRow(u: UploadInfo): HTMLElement {
    const row = document.createElement("div");
    row.className = `upload-row status-${u.status}`;

    const nameCell = document.createElement("div");
    nameCell.className = "upload-name";
    nameCell.textContent = u.name;

    const sizeCell = document.createElement("div");
    sizeCell.className = "upload-size";
    let sizeText = `${formatBytes(u.uploaded)} / ${formatBytes(u.size)}`;
    if (u.speed !== undefined && u.speed > 0) {
      sizeText += ` · ${formatSpeed(u.speed)}`;
      if (u.eta) sizeText += ` · ETA ${u.eta}`;
    }
    sizeCell.textContent = sizeText;

    const progCell = document.createElement("div");
    progCell.className = "upload-progress";
    const bar = document.createElement("div");
    bar.className = "progress-bar";
    const fill = document.createElement("div");
    fill.className = "progress-fill";
    fill.style.width = `${clamp(u.progress, 0, 100)}%`;
    bar.appendChild(fill);
    progCell.appendChild(bar);

    row.appendChild(nameCell);
    row.appendChild(sizeCell);
    row.appendChild(progCell);
    return row;
  }

  // -- actions ------------------------------------------------------------

  private activeToken(): string {
    return this.state.token || this.state.session?.token || "";
  }

  async downloadFile(file: FileInfo): Promise<void> {
    const token = this.activeToken();
    if (!token) return;
    // Segment-encode the file name (encode each path segment separately)
    const encodedName = file.name.split("/").map(encodeURIComponent).join("/");
    const url = `${this.state.baseUrl}/${token}/download/${file.index}/${encodedName}`;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    try {
      file.status = "downloading";
      file.progress = 0;
      this.renderFiles();

      const res = await fetch(url);
      if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
      }
      const contentLength = Number(res.headers?.get("Content-Length"));
      if ((!Number.isFinite(file.size) || file.size <= 0) && Number.isFinite(contentLength) && contentLength > 0) {
        file.size = contentLength;
      }

      // Streaming download with progress when supported
      if (res.body && typeof res.body.getReader === "function") {
        reader = res.body.getReader();
        const chunks: BlobPart[] = [];
        let received = 0;
        let lastTime = Date.now();
        let lastReceived = 0;
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          const chunkCopy = new Uint8Array(value.byteLength);
          chunkCopy.set(value);
          chunks.push(chunkCopy.buffer);
          received += value.length;
          if (file.size > 0) {
            file.progress = clamp(Math.round((received / file.size) * 100), 0, 100);
          }
          const now = Date.now();
          const dt = (now - lastTime) / 1000;
          if (dt > 0) {
            file.speed = Math.round((received - lastReceived) / dt);
            const remaining = Math.max(0, (file.size || 0) - received);
            file.eta = formatETA(remaining, file.speed);
          }
          lastReceived = received;
          lastTime = now;
          this.renderFiles();
        }
        const blob = new Blob(chunks);
        const downloadUrl = URL.createObjectURL(blob);
        try {
          const a = document.createElement("a");
          a.href = downloadUrl;
          a.download = extractFilename(file.name);
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
        } finally {
          URL.revokeObjectURL(downloadUrl);
        }
      } else {
        const blob = await res.blob();
        const downloadUrl = URL.createObjectURL(blob);
        try {
          const a = document.createElement("a");
          a.href = downloadUrl;
          a.download = extractFilename(file.name);
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
        } finally {
          URL.revokeObjectURL(downloadUrl);
        }
      }

      file.status = "done";
      file.progress = 100;
      file.downloaded = true;
      file.speed = undefined;
      file.eta = undefined;
    } catch (err) {
      if (reader) {
        try { await reader.cancel(); } catch { /* stream already failed or closed */ }
      }
      file.status = "error";
      file.speed = undefined;
      file.eta = undefined;
      console.error("lanbeam: download failed:", err);
    } finally {
      this.renderFiles();
    }
  }

  async openFile(file: FileInfo): Promise<void> {
    const token = this.activeToken();
    if (!token) return;
    const encodedName = file.name.split("/").map(encodeURIComponent).join("/");
    const url = `${this.state.baseUrl}/${token}/download/${file.index}/${encodedName}`;
    window.open(url, "_blank");
  }

  async downloadAll(): Promise<void> {
    const token = this.activeToken();
    if (!token) return;
    const url = `${this.state.baseUrl}/${token}/download-all.zip`;
    try {
      const res = await fetch(url);
      if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
      }
      const blob = await res.blob();
      const downloadUrl = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = downloadUrl;
      a.download = "lanbeam.zip";
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(downloadUrl);
    } catch (err) {
      console.error("lanbeam: download-all failed:", err);
      alert("Failed to download all files.");
    }
  }

  // Handle multi-file upload from an <input type=file> or drag-and-drop.
  async queueUploads(files: FileList | null): Promise<void> {
    if (!files || files.length === 0) return;
    for (const file of Array.from(files)) {
      try {
        await this.uploadFileWithProgress(file);
      } catch (err) {
        console.error("lanbeam: upload failed:", err);
      }
    }
  }

  /** Upload a single File object with XHR progress tracking and live speed/ETA. */
  uploadFileWithProgress(file: File): Promise<void> {
    const up: UploadInfo = {
      name: file.name,
      size: file.size,
      uploaded: 0,
      progress: 0,
      status: "uploading",
    };
    this.state.uploads.push(up);
    this.renderUploads();

    return new Promise((resolve, reject) => {
      const token = this.activeToken();
      if (!token) {
        up.status = "error";
        this.renderUploads();
        reject(new Error("no session token"));
        return;
      }
      const url = `${this.state.baseUrl}/${token}/upload?name=${encodeURIComponent(up.name)}`;
      const xhr = new XMLHttpRequest();
      try {
        xhr.open("POST", url, true);
      } catch (err) {
        up.status = "error";
        this.renderUploads();
        reject(err);
        return;
      }

      let lastBytes = 0;
      let lastTime = Date.now();

      xhr.upload.addEventListener("progress", (ev) => {
        if (ev.lengthComputable) {
          up.uploaded = ev.loaded;
          up.size = ev.total;
          up.progress = ev.total > 0 ? clamp(Math.round((ev.loaded / ev.total) * 100), 0, 100) : 0;

          // Compute live speed/ETA
          const now = Date.now();
          const dt = (now - lastTime) / 1000;
          if (dt > 0 && ev.total > 0) {
            const delta = ev.loaded - lastBytes;
            up.speed = Math.round(delta / dt);
            const remaining = Math.max(0, (up.size || 0) - ev.loaded);
            up.eta = formatETA(remaining, up.speed);
          }
          lastBytes = ev.loaded;
          lastTime = now;

          this.renderUploads();
        }
      });

      xhr.addEventListener("load", () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          up.status = "done";
          up.progress = 100;
          up.uploaded = up.size;
          up.speed = undefined;
          up.eta = undefined;
          this.renderUploads();
          resolve();
        } else {
          up.status = "error";
          up.speed = undefined;
          up.eta = undefined;
          this.renderUploads();
          reject(new Error(`Upload failed: ${xhr.status}`));
        }
      });

      xhr.addEventListener("error", () => {
        up.status = "error";
        up.speed = undefined;
        up.eta = undefined;
        this.renderUploads();
        reject(new Error("Upload network error"));
      });

      xhr.addEventListener("abort", () => {
        up.status = "error";
        up.speed = undefined;
        up.eta = undefined;
        this.renderUploads();
        reject(new Error("Upload aborted"));
      });

      try {
        xhr.send(file);
      } catch (err) {
        up.status = "error";
        up.speed = undefined;
        up.eta = undefined;
        this.renderUploads();
        reject(err);
      }
    });
  }

  async sendTextFromUI(text: string): Promise<void> {
    try {
      await this.api.sendText(text);
    } catch (err) {
      console.error("lanbeam: send text failed:", err);
      alert("Failed to send text.");
    }
  }

  private async copyText(value: string, toastMessage: string): Promise<void> {
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      this.showToast(toastMessage);
    } catch {
      // Fallback for older browsers.
      const ta = document.createElement("textarea");
      ta.value = value;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
        this.showToast(toastMessage);
      } finally {
        document.body.removeChild(ta);
      }
    }
  }

  async copyLink(): Promise<void> {
    await this.copyText(
      this.state.session?.url || "",
      "Link copied to clipboard"
    );
  }

  async copySharedText(): Promise<void> {
    await this.copyText(
      this.state.session?.shared_text || "",
      "Text copied to clipboard"
    );
  }

  async stopSharing(): Promise<void> {
    try {
      await this.api.stopSession();
    } catch (err) {
      console.error("lanbeam: stop session failed:", err);
    }
    this.stop();
    if (this.state.session) {
      this.state.session.active = false;
      this.render();
    }
  }

  async shareText(text: string): Promise<void> {
    if (!text.trim()) return;
    await this.sendTextFromUI(text);
    this.showToast("Text shared");
  }

  private showToast(msg: string): void {
    const toast = this.el("toast");
    toast.textContent = msg;
    toast.classList.remove("hidden");
    toast.classList.add("show");
    window.setTimeout(() => {
      toast.classList.remove("show");
      window.setTimeout(() => toast.classList.add("hidden"), 300);
    }, 2000);
  }

  private setText(id: string, text: string): void {
    const el = this.el(id);
    if (el) el.textContent = text;
  }

  // -- wiring -------------------------------------------------------------

  /** Attach all DOM event listeners after the HTML is mounted. */
  wire(root: HTMLElement): void {
    // Theme toggle.
    const themeBtn = root.querySelector('[data-action="theme"]');
    if (themeBtn) {
      themeBtn.addEventListener("click", () => this.toggleTheme());
    }

    // Copy link buttons (multiple).
    root.querySelectorAll('[data-action="copy-link"]').forEach((btn) => {
      btn.addEventListener("click", () => this.copyLink());
    });

    // Copy shared text buttons (multiple).
    root.querySelectorAll('[data-action="copy-text"]').forEach((btn) => {
      btn.addEventListener("click", () => this.copySharedText());
    })

    // Download All button.
    const dlAllBtn = root.querySelector('[data-action="download-all"]');
    if (dlAllBtn) {
      dlAllBtn.addEventListener("click", () => this.downloadAll());
    }

    // File input change → multi-file upload with progress.
    const fileInput = root.querySelector(
      '#file-input'
    ) as HTMLInputElement | null;
    if (fileInput) {
      fileInput.addEventListener("change", () => {
        this.queueUploads(fileInput.files);
        fileInput.value = "";
      });
    }

    // Drag-and-drop upload on the drop zone.
    const dropZone = root.querySelector('[data-drop-zone]') as
      | HTMLElement
      | null;
    if (dropZone) {
      const prevent = (e: DragEvent) => {
        e.preventDefault();
        e.stopPropagation();
      };
      dropZone.addEventListener("dragenter", prevent);
      dropZone.addEventListener("dragover", prevent);
      dropZone.addEventListener("dragleave", prevent);
      dropZone.addEventListener("drop", (e) => {
        prevent(e);
        const dt = e.dataTransfer;
        if (dt?.files) {
          this.queueUploads(dt.files);
        }
      });
    }

    // Text share form.
    const textForm = root.querySelector('#text-share-form') as
      | HTMLFormElement
      | null;
    const textInput = root.querySelector('#text-share-input') as
      | HTMLInputElement
      | null;
    if (textForm && textInput) {
      textForm.addEventListener("submit", (e) => {
        e.preventDefault();
        this.shareText(textInput.value);
        textInput.value = "";
      });
    }

    // Stop sharing button.
    const stopBtn = root.querySelector('[data-action="stop"]') as
      | HTMLButtonElement
      | null;
    if (stopBtn) {
      stopBtn.addEventListener("click", () => {
        void this.stopSharing();
      });
    }
  }
}
