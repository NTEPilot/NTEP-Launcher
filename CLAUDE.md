# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在本仓库中工作时提供指引。

**必须使用中文与用户交流。**

## 项目概述

NTEP Launcher 是 NTEPilot 的跨平台（Windows/macOS/Linux）桌面启动器，基于 **Tauri 2 + Rust** 构建。它通过内嵌的 `uv` 二进制文件管理独立的 Python 3.14.3 环境，同步 `pyproject.toml` / `uv.lock` 依赖，直接启动 Python 后端入口 `main.py`，并打开固定 WebUI 地址 `http://127.0.0.1:9150`。

启动器不再执行 Git 更新，不再依赖 旧部署脚本或旧部署配置。

## 构建与开发命令

```bash
cargo build
cargo build --release
cargo tauri dev
cargo test
cargo check
```

构建脚本（`build.rs`）可通过 `NTEP_BOOTSTRAP_UV` 环境变量指定要内嵌的 `uv` 二进制文件。本地开发不设置时使用空占位符，启动器会在运行时从 PATH 中查找 `uv` 或通过环境变量 `UV` 指定。

## 启动参数

| 参数 | 别名 | 说明 |
|---|---|---|
| `--lang <locale>` | `--locale <locale>` | 覆盖系统语言，支持 `zh-CN`、`zh-TW`、`ja`、`en`。 |
| `--preview-crash` | `--preview-error`、`--crash-preview`、`--error-preview` | 模拟启动失败，停留在错误页面以检查 UI。 |

示例：

```bash
ntep-launcher --lang ja
ntep-launcher --preview-crash
```

## 架构

### 源码模块

| 模块 | 职责 |
|---|---|
| `main.rs` | Tauri 应用入口、窗口管理、启动画面、托盘、时间炸弹、自定义标题栏、错误页面 |
| `backend.rs` | 启动/终止 `main.py` 子进程、固定端口 9150、端口占用清理、后端生命周期 |
| `setup.rs` | 工作区检查、Python/uv 准备、`uv sync` 依赖同步、`.venv` 清理 |
| `notify.rs` | SSE 通知流、平台原生桌面通知 |
| `i18n.rs` | 国际化和 `--lang` 参数解析 |
| `window_util.rs` | Windows 子进程窗口控制 |

### 运行时流程

1. `main()` 初始化日志，固定 WebUI 端口为 `9150`。
2. 创建 Tauri 应用，显示 splash 窗口（`ntep-splash://` 或 Windows 上的 `http://ntep-splash.localhost/`）。
3. 后台线程执行 `setup_workspace()`：
   - 检查工作区包含 `main.py`、`pyproject.toml`、`uv.lock`。
   - 通过 uv managed python 准备 Python 3.14.3。
   - 创建可重定位 `.venv`，复制 uv。
   - 执行 `uv sync --frozen --no-dev --no-install-project`。
4. `ManagedBackend::new()` 使用 `.venv` Python 直接启动 `main.py`，设置 `NTEP_LAUNCHER_PID`。
5. 等待 `http://127.0.0.1:9150` 可连接，启动 SSE 通知流，销毁 splash 并显示主窗口。

### 自定义 URI 协议

- `ntep-splash://`：内嵌启动画面，进度通过 `window.__NTEP_SPLASH_UPDATE()` 更新。
- `ntep-error://`：后端连接失败页面，每秒自动重试连接。

### 工作区结构

- Windows/Linux：启动器与后端文件位于 `NTEPilot` 根目录。
- macOS：启动器位于 `NTEP Launcher.app/Contents/MacOS/ntep-launcher`，后端位于 `NTEP Launcher.app/Contents/NTEPilot`。

### 关键常量

- 默认 WebUI 端口：`9150`
- 后端入口：`main.py`
- Python 版本：`3.14.3`
- 后端端口等待超时：60 秒
- 后端连接检查超时：500 毫秒
- 通知流断线重连间隔：3 秒
- Python 下载镜像可通过 `UV_PYTHON_INSTALL_MIRROR` 覆盖

### CI/CD

GitHub Actions 在 tag push 或手动触发时：
- 构建 Rust/Tauri 启动器。
- 拉取 `NTEPilot/NTEPilot` 的同名 tag。
- 打包启动器、后端源码、Python `.venv` 和 uv。
- 输出 `NTEP-Launcher-${runner.os}-${arch}.tar.xz`。
