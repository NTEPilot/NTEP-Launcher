**| English | [简体中文](README.md) |**

NTEP Launcher
===

NTEP Launcher is the cross-platform desktop launcher for NTEPilot, built with Tauri 2 and Rust.

Usage
---
Download the archive for your OS and CPU architecture from Releases, extract it, then run:
- Windows: `ntep-launcher.exe`. Windows 7, 8, and 10 require [WebView2](https://developer.microsoft.com/en-us/Microsoft-edge/webview2).
- macOS: `NTEP Launcher.app`. If macOS blocks the unsigned app, run `xattr -dr com.apple.quarantine "NTEP Launcher.app"` in Terminal.
- Linux: `ntep-launcher`. The app requires `libwebkit2gtk-4.1` and a recent `glibc`.

Runtime Behavior
---
1. The launcher checks that the backend workspace contains `main.py`, `pyproject.toml`, and `uv.lock`.
2. It creates a relocatable `.venv` with the embedded uv or a uv found on PATH.
3. It runs `uv sync --frozen --no-dev --no-install-project`.
4. It starts `main.py` with the `.venv` Python and passes no arguments.
5. It waits for and opens `http://127.0.0.1:9150`.

Directory Structure
---
Backend workspace:
- Windows/Linux: `NTEPilot`
- macOS: `NTEP Launcher.app/Contents/NTEPilot`

Launcher:
- Windows: `NTEPilot/ntep-launcher.exe`
- Linux: `NTEPilot/ntep-launcher`
- macOS: `NTEP Launcher.app/Contents/MacOS/ntep-launcher`

Runtime:
- Python/uv: `.venv`

CI/CD
---
On tag builds, GitHub Actions fetches the same tag from `https://github.com/NTEPilot/NTEPilot` and packages it with the launcher.

Archives are named `NTEP-Launcher-${runner.os}-${arch}.tar.xz`.

License
---
This project uses GPLv3. Check upstream projects for dependency licenses.
