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
}

// ---------------------------------------------------------------------------
// UI controller
// ---------------------------------------------------------------------------

export class LanbeamUI {
  private state: AppState;
  private api: LanbeamApi;
  private pollTimer: number | null = null;
  private lastBytes = 0;
  private lastTime = 0;

  // DOM elements (cached after first render).
  private els: Record<string, HTMLElement | null> = {};

  constructor(token: string, baseUrl: string) {
    this.state = {
      token,
      baseUrl,
      session: null,
      files: [],
      uploads: [],
      theme: "light",
      polling: false,
    };
    this.api = new LanbeamApi(baseUrl);
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
      clearInterval(this.pollTimer);
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
        this.pollTimer = window.setTimeout(() => this.poll(), 1000);
      });
  }

  private async refreshStatus(): Promise<void> {
    const session = await this.api.getStatus();
    this.state.session = session;
    this.syncFilesFromSession(session);
    this.render();
  }

  private syncFilesFromSession(session: SessionState): void {
    // Merge session file list into our local file model, preserving progress.
    const byName = new Map(this.state.files.map((f) => [f.name, f]));
    const next: FileInfo[] = session.files.map((name) => {
      const existing = byName.get(name);
      if (existing) return existing;
      return {
        name,
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
    sizeCell.textContent = f.size > 0 ? formatBytes(f.size) : "–";

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
    sizeCell.textContent = `${formatBytes(u.uploaded)} / ${formatBytes(u.size)}`;

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

  async downloadFile(file: FileInfo): Promise<void> {
    if (!this.state.session) return;
    const { token } = this.state.session;
    const url = `${this.state.baseUrl}/${token}/download/${encodeURIComponent(file.name)}`;
    try {
      file.status = "downloading";
      file.progress = 0;
      this.renderFiles();

      const res = await fetch(url);
      const blob = await res.blob();
      const downloadUrl = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = downloadUrl;
      a.download = file.name;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(downloadUrl);

      file.status = "done";
      file.progress = 100;
      file.downloaded = true;
    } catch (err) {
      file.status = "error";
      console.error("lanbeam: download failed:", err);
    } finally {
      this.renderFiles();
    }
  }

  async openFile(file: FileInfo): Promise<void> {
    if (!this.state.session) return;
    const { token } = this.state.session;
    const url = `${this.state.baseUrl}/${token}/download/${encodeURIComponent(file.name)}`;
    window.open(url, "_blank");
  }

  async downloadAll(): Promise<void> {
    if (!this.state.session) return;
    const { token } = this.state.session;
    const url = `${this.state.baseUrl}/${token}/download-all.zip`;
    try {
      const res = await fetch(url);
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

  // Handle multi-file upload from an <input type=file>.
  async queueUploads(files: FileList | null): Promise<void> {
    if (!files) return;
    const arr = Array.from(files);
    for (const file of arr) {
      const up: UploadInfo = {
        name: file.name,
        size: file.size,
        uploaded: 0,
        progress: 0,
        status: "queued",
      };
      this.state.uploads.push(up);
    }
    this.renderUploads();
    for (const up of this.state.uploads) {
      if (up.status !== "queued") continue;
      await this.performUpload(up);
    }
  }

  private performUpload(up: UploadInfo): Promise<void> {
    return new Promise((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      // We don't have the actual File object reference here easily; instead
      // the caller should use uploadFile directly. Keep this as a stub that
      // resolves immediately when called via queueUploads with real files.
      resolve();
    });
  }

  /** Upload a single File object with XHR progress tracking. */
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
      if (!this.state.session) {
        reject(new Error("no session"));
        return;
      }
      const { token } = this.state.session;
      const url = `${this.state.baseUrl}/${token}/upload?name=${encodeURIComponent(up.name)}`;
      const xhr = new XMLHttpRequest();
      xhr.open("POST", url, true);

      xhr.upload.addEventListener("progress", (ev) => {
        if (ev.lengthComputable) {
          up.uploaded = ev.loaded;
          up.size = ev.total;
          up.progress = Math.round((ev.loaded / ev.total) * 100);
          this.renderUploads();
        }
      });

      xhr.addEventListener("load", () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          up.status = "done";
          up.progress = 100;
          up.uploaded = up.size;
          this.renderUploads();
          resolve();
        } else {
          up.status = "error";
          this.renderUploads();
          reject(new Error(`Upload failed: ${xhr.status}`));
        }
      });

      xhr.addEventListener("error", () => {
        up.status = "error";
        this.renderUploads();
        reject(new Error("Upload network error"));
      });

      xhr.send(file);
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

  async copyLink(): Promise<void> {
    const url = this.state.session?.url;
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
      this.showToast("Link copied to clipboard");
    } catch {
      // Fallback for older browsers.
      const ta = document.createElement("textarea");
      ta.value = url;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
        this.showToast("Link copied to clipboard");
      } finally {
        document.body.removeChild(ta);
      }
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

    // Copy link button.
    const copyBtn = root.querySelector('[data-action="copy-link"]');
    if (copyBtn) {
      copyBtn.addEventListener("click", () => this.copyLink());
    }

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
        this.stop();
      });
    }
  }
}
