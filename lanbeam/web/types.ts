// Shared types for the lanbeam mobile web frontend.
// Mirrors the Rust SessionState shape served by GET /<token>/status.

export type LanbeamMode = "send" | "receive" | "text";

export interface SessionState {
  mode: LanbeamMode;
  url: string;
  files: string[];
  downloads_completed: number;
  uploads_received: number;
  shared_text: string | null;
  remaining_seconds: number;
  active: boolean;
}

export interface FileInfo {
  name: string;
  size: number;
  downloaded: boolean;
  progress: number; // 0..100
  status: "pending" | "downloading" | "done" | "error";
}

export interface UploadInfo {
  name: string;
  size: number;
  uploaded: number;
  progress: number; // 0..100
  status: "queued" | "uploading" | "done" | "error";
}

export interface AppState {
  token: string;
  baseUrl: string;
  session: SessionState | null;
  files: FileInfo[];
  uploads: UploadInfo[];
  theme: "light" | "dark";
  polling: boolean;
}
