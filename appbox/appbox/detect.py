"""Package format detection and application ID normalization."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Literal

PackageType = Literal["appimage", "rpm"]

RPM_MAGIC = b"\xed\xab\xee\xdb"
ELF_MAGIC = b"\x7fELF"
APPIMAGE_SIGNATURES = (b"AI\x02", b"AI\x01")

_ARCH_TOKENS = {
    "x86_64",
    "x86-64",
    "amd64",
    "aarch64",
    "arm64",
    "i386",
    "i686",
    "noarch",
    "linux",
}
_VERSION_RE = re.compile(r"^v?\d+(?:[._-]\d+)*(?:[._-](?:rc|beta|alpha)\d*)?$")


class UnsupportedPackageError(ValueError):
    """Raised when a file is neither an AppImage nor an RPM package."""


def detect_package_type(path: Path) -> PackageType:
    """Detect whether *path* is an AppImage or RPM package."""
    if not path.is_file():
        raise FileNotFoundError(f"Package file not found: {path}")

    with path.open("rb") as handle:
        header = handle.read(64)

    if header.startswith(RPM_MAGIC):
        return "rpm"

    suffix = path.suffix.lower()
    if header.startswith(ELF_MAGIC):
        if len(header) >= 11 and header[8:11] in APPIMAGE_SIGNATURES:
            return "appimage"
        if suffix == ".appimage":
            return "appimage"

    if suffix == ".rpm":
        return "rpm"
    if suffix == ".appimage":
        return "appimage"

    raise UnsupportedPackageError(
        f"Unsupported package format for '{path.name}' (expected .AppImage or .rpm)"
    )


def slugify_app_id(raw: str) -> str:
    """Normalize an application name or stem into a safe lowercase slug."""
    cleaned = re.sub(r"[^a-z0-9]+", "-", raw.lower()).strip("-")
    return cleaned or "app"


def infer_app_id(source_path: Path, preferred_name: str | None = None) -> str:
    """Infer a stable application ID from *preferred_name* or *source_path*."""
    if preferred_name and preferred_name.strip():
        return slugify_app_id(preferred_name)

    stem = source_path.name
    for ext in (".appimage", ".rpm"):
        if stem.lower().endswith(ext):
            stem = stem[: -len(ext)]
            break

    # Split on hyphens/dots when trailing tokens are architecture or version numbers.
    raw_parts = stem.replace(".", "-").replace("_", "-").split("-")
    trimmed = list(raw_parts)
    while len(trimmed) > 1:
        tail = trimmed[-1].lower()
        # Handle split x86_64 -> x86, 64
        if len(trimmed) >= 2 and trimmed[-2].lower() == "x86" and tail == "64":
            trimmed = trimmed[:-2]
            continue
        if tail in _ARCH_TOKENS or _VERSION_RE.match(tail) or tail.isdigit():
            trimmed.pop()
            continue
        break

    if not trimmed:
        trimmed = raw_parts or [stem]
    return slugify_app_id("-".join(trimmed))
