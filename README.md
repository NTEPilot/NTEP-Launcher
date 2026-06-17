**| [English](README_en.md) | 简体中文 |**

NTEP Launcher
===

NTEP Launcher 是 NTEPilot 的跨平台桌面启动器，基于 Tauri 2 + Rust 构建。

使用方法
---
从 Releases 下载对应系统和 CPU 架构的压缩包，解压后运行：
- Windows: 打开 `ntep-launcher.exe`。Windows 7、8、10 需要先安装 [WebView2](https://developer.microsoft.com/zh-cn/microsoft-edge/webview2)。
- macOS: 打开 `NTEP Launcher.app`。如果系统拦截未签名应用，可在终端运行 `xattr -dr com.apple.quarantine "NTEP Launcher.app"`。
- Linux: 运行 `ntep-launcher`。程序依赖 `libwebkit2gtk-4.1` 和较新的 `glibc`。

运行行为
---
1. 启动器在后端根目录检查 `main.py`、`pyproject.toml`、`uv.lock`。
2. 使用内置或系统 PATH 中的 `uv` 创建可重定位 `.venv`。
3. 执行 `uv sync --frozen --no-dev --no-install-project` 同步 Python 依赖。
4. 直接执行 `.venv` 中的 Python 启动 `main.py`，不传任何参数。
5. 等待并打开固定地址 `http://127.0.0.1:9150`。

目录结构
---
后端工作区：
- Windows/Linux: `NTEPilot`
- macOS: `NTEP Launcher.app/Contents/NTEPilot`

启动器：
- Windows: `NTEPilot/ntep-launcher.exe`
- Linux: `NTEPilot/ntep-launcher`
- macOS: `NTEP Launcher.app/Contents/MacOS/ntep-launcher`

运行环境：
- Python/uv: `.venv`

CI/CD
---
发布 tag 时，GitHub Actions 会拉取 `https://github.com/NTEPilot/NTEPilot` 的同名 tag，与启动器一起打包。

产物命名为 `NTEP-Launcher-${runner.os}-${arch}.tar.xz`。

许可证
---
本项目沿用 GPLv3。依赖库许可证请查看各自上游项目。
