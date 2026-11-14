# Utilites

A collection of lightweight desktop utilities for Linux.

## Utilities

| Utility | Stack | Description | Docs |
|---|---|---|---|
| **rclip** | GNOME Shell (GJS) + Python 3 | Windows `Win + V` style clipboard manager (text & images + auto-paste) for GNOME Wayland | [rclip/README.md](rclip/README.md) |
| **appbox** | Python 3 (stdlib) | User-space `.AppImage` & `.rpm` desktop integrator (CLI + GNOME Files right-click) | [appbox/README.md](appbox/README.md) |

---

## CI/CD & Releases

- **CI (`.github/workflows/ci.yml`):** Runs on pushes and pull requests to `main` — tests `rclip` (`gjs` store tests, `unittest`, `glib-compile-schemas`, `shellcheck`) and `appbox` (`unittest`, `compileall`, `shellcheck`, `zipapp`).
- **Releases (`.github/workflows/release.yml`):** Triggered by per-utility SemVer tags or manual `workflow_dispatch`:
  ```bash
  # Release rclip (must match rclip/rclip_cli/__init__.py __version__)
  git tag rclip-v0.2.0 && git push origin rclip-v0.2.0

  # Release appbox (must match appbox/appbox/__init__.py __version__)
  git tag appbox-v0.1.0 && git push origin appbox-v0.1.0
  ```
