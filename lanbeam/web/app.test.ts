// Unit tests for the pure helper functions in web/app.ts.
// Run with: bun test

import {
  describe,
  expect,
  it,
} from "bun:test";

import {
  formatBytes,
  formatSpeed,
  formatETA,
  extractFilename,
  clamp,
} from "./app.ts";

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
