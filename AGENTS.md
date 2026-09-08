---
description:
alwaysApply: true
---

# AGENTS.md

> **重要：必须使用中文交流。** 所有回复、注释、文档、提交信息均使用简体中文。代码中的变量名、函数名、结构体名使用英文。

本文件为 AI Agent 与开发者在本仓库（`AzurNext-Launcher`）中工作时的**唯一权威核心规范与架构设计基准文档**。所有工程规范、架构速查、编码模式、命令与发版流程均统一在本文件中维护。

---

## 项目概述

**AzurNext Launcher** 是 AzurNext 的跨平台（Windows / macOS / Linux）桌面原生启动器与外壳程序，基于 **Tauri 2 + Rust** 构建。
它负责内嵌与管理独立的 Python 3.14.6 运行时环境（基于 `uv`），管理本地仓库更新与依赖同步，启动并监控本地 Python WebUI 后端（`gui.py`），并提供原生系统集成（启动画面、系统托盘、多语言原生通知、自定义无边框标题栏等）。

**核心设计约束**：
1. **极速启动（秒开优先）**：日常启动必须做到秒开（2~3 秒进入主界面）。启动流程严禁默认加入可能阻塞的外部网络请求与 Git 重试；网络更新应由用户手动触发或后台静默拉取。
2. **环境自包含**：通过 `uv` 管理独立的 Python 虚拟环境（`.venv`），不依赖宿主全局 Python。
3. **零进程/端口泄漏**：Launcher 进程退出时，必须严密清理所有子进程树（`ALAS_LAUNCHER_PID` 绑定），并在启动前自动回收占用目标端口的僵尸进程。
4. **轻量与原生**：前端界面直接加载本地 WebUI，原生外壳不引入庞大的前端工程技术栈，保持纯粹的 Rust + 原生 DOM 注入。

---

## 开发与构建命令

```bash
cargo check                     # 极速代码类型与语法检查（日常修改后优先使用）
cargo test --no-run             # 编译验证测试套件（避免直接执行时的 UAC 管理员提权限制）
cargo test                      # 运行单元测试
cargo test test_name_here       # 运行单个测试
cargo build                     # 构建 Debug 版本
cargo build --release           # 构建 Release 版本（启用 LTO 优化）
cargo tauri dev                 # 启动 Tauri 开发调试模式
```

> **注意**：
> - Windows 下启动器内置了管理员权限 Manifest，非提权环境下直接执行生成的可执行文件会报 `os error 740`。日常自动化验证推荐运行 `cargo check` 与 `cargo test --no-run`。
> - 构建脚本（`build.rs`）需要 `ALAS_BOOTSTRAP_UV` 环境变量指向要内嵌的 `uv` 二进制文件。本地开发不设置时使用空占位符，启动器会在运行时从 PATH 中查找 `uv` 或通过环境变量 `UV` 指定。

---

## 启动命令行参数

启动器支持以下命令行参数（Windows 下使用 `/` 前缀同样有效）：

| 参数 | 别名 | 说明 |
|---|---|---|
| `--lang <locale>` | `--locale <locale>` | 覆盖系统语言，强制使用指定语言显示界面。支持的值：`zh-CN`（简体中文）、`zh-TW`（繁体中文）、`ja`（日语）、`en`（英语）。也支持 `--lang=zh-CN` 形式。 |
| `--preview-no-update` | `--skip-update`、`--no-update`、`--disable-update` | 跳过仓库更新，直接使用本地文件启动。用于开发调试。 |
| `--preview-crash` | `--preview-error`、`--crash-preview`、`--error-preview` | 模拟启动失败，停留在错误页面以检查 UI 样式。隐含 `--preview-no-update`。 |

示例：
```bash
# 以日语界面启动
alas-launcher --lang ja

# 以英语界面启动，跳过更新
alas-launcher --lang en --skip-update

# 预览错误页面
alas-launcher --preview-crash
```

---

## 架构与核心源码模块

### 源码模块清单（`src/`）

| 模块 | 行数 | 职责说明 |
|---|---|---|
| `main.rs` | ~2140 | Tauri 应用入口、窗口管理、启动画面、系统托盘、时间炸弹、自定义标题栏注入、错误页面、IPC 命令分发 |
| `backend.rs` | ~330 | 启动/终止 `gui.py` 子进程、端口扫描与占用进程清理、后端生命周期管理与孤儿进程清理 |
| `setup.rs` | ~1460 | 运行环境配置：Python/uv/adb/git 安装与检查、git 更新拉取、uv 依赖同步、deploy.yaml 配置迁移、运行时清理 |
| `notify.rs` | ~320 | SSE 通知流（`/api/notify_stream`）、平台原生桌面通知（Windows Toast / Linux notify-rust / macOS Tauri 插件） |
| `i18n.rs` | ~55 | 国际化管理：系统语言检测、`--lang` 启动参数解析、locale 设置与映射 |
| `window_util.rs` | ~45 | Windows `CREATE_NO_WINDOW` trait，控制子进程启动时是否创建控制台窗口 |

### 运行时流程与生命周期（Runtime Flow）

1. **初始化**：`main()` 初始化日志（写入 `log/{date}_launcher.txt`），读取 `config/deploy.yaml` 获取 WebUI 端口与配置。
2. **启动画面**：创建 Tauri 应用，展示 Splash 启动画面窗口（自定义协议 `alas-splash://`），预先隐藏主窗口。
3. **环境就绪与仓库同步**：后台线程执行 `setup_alas_repo()`：
   - `ensure_runtime_tools()`：下载 Python 3.14.6（通过 uv managed python）、创建可重定位 `.venv`、复制 uv/adb/git 到 `.venv`；检测到 `.venv` 的 Python 低于 3.14.5 时会自动删除并重建环境；
   - `git_update()`：根据更新模式调用 Python 脚本 `deploy.git.GitManager` 拉取最新代码（最多重试 20 次）；
   - `uv_sync_project()`：执行 `uv sync --frozen --no-dev --no-install-project` 同步依赖。
4. **后端启动与监控**：`ManagedBackend::new()` 启动 `gui.py`，绑定 `ALAS_LAUNCHER_PID` 环境变量，轮询等待端口就绪（60 秒超时）。
5. **主窗口切换与通信**：建立 SSE 通知流监听，销毁 Splash 窗口，显示并聚焦主窗口。

### 自定义协议与窗口管理

- **自定义 URI 协议**：
  - `alas-splash://`（Windows/Android 映射为 `http://alas-splash.localhost/`）：内嵌启动画面 HTML/CSS/JS，进度通过 `window.__ALAS_SPLASH_UPDATE()` 回调动态刷新；
  - `alas-error://`（Windows/Android 映射为 `http://alas-error.localhost/`）：后端连接失败展示页，每秒自动检测重试。
- **自定义标题栏与原生功能注入**：
  - Windows 和 Linux 移除原生窗口装饰（`set_decorations(false)`），通过注入脚本生成无边框红绿灯与拖拽区；macOS 保留原生标题栏；
  - 注入脚本重载 `window.saveAs` 路由到 Tauri `save_as` 命令；
  - 监听并拦截浏览器后退事件（`history.pushState` + `popstate`）。
- **跨平台窗口关闭行为**：
  - **Windows**：弹出退出确认对话框（退出应用 / 最小化到系统托盘），最小化时主动调用 `destroy()` 销毁主窗口以彻底释放 WebView 内存占用，托盘唤醒时重新按需创建；
  - **macOS**：最小化到托盘，切换 `ActivationPolicy` 为 `Accessory`（自动隐藏 Dock 图标），托盘唤醒恢复时切换回 `Regular`；
  - **Linux**：直接隐藏主窗口。
- **零进程泄漏与端口清理保障**：
  - **端口占用清理**：`backend.rs` 在启动前扫描目标端口（Windows 用 `netstat -ano -p tcp`，Unix 用 `lsof -nP -iTCP:{port} -sTCP:LISTEN -t`），使用 `sysinfo` 强力终止占用端口的僵尸进程（最长等待 5 秒）；
  - **孤儿进程级联清理**：`ManagedBackend` 的 `Drop` 实现会遍历全局进程环境变量，凡包含 `ALAS_LAUNCHER_PID={当前PID}` 的子进程均予以彻底 Kill，杜绝子进程遗留；单实例新进程唤醒旧窗口时亦会自动清理失效孤儿进程。
- **配置与安全机制**：
  - **部署配置自动迁移**：`setup.rs` 中的 `migrate_dependency_config()` 在启动时自动检查 `config/deploy.yaml`，重定向 Python/Git/Adb 路径至 `.venv` 并清理已弃用配置；
  - **时间炸弹机制**：`Cargo.toml` 中支持 `[package.metadata.alas-launcher.time-bomb]`，启用时启动器通过 HTTP 请求校验远端网络时间（`Date` 请求头），过期则弹窗终止运行；
  - **Tauri IPC 基础命令**：`save_as`、`download_today_gui_log`、`download_today_launcher_log`、`retry_backend_connection`、`window_hide`、`window_minimize`、`window_toggle_maximize`、`window_close`、`window_start_dragging`、`window_is_maximized`。

### 关键常量与默认配置

- 默认 WebUI 端口：`22267`
- Python 版本：`3.14.6`（`setup.rs` 中 `PYTHON_VERSION`）
- Git 更新最大重试：20 次，间隔 1 秒
- 后端端口等待超时：60 秒
- 后端连接检查超时：500 毫秒
- 通知流断线重连间隔：3 秒
- `UV_PYTHON_INSTALL_MIRROR`：默认优先使用 npmmirror 加速 Python standalone 下载，并以 python-standalone.org 作为备用源

---

## 核心编码规范

### 1. 语言与注释规范

- **全中文交流与注释**：代码注释、文档说明、Git Commit、PR 描述必须全部使用**简体中文**。
- **英文标识符**：Rust 中的模块名、函数名、变量名、结构体名、枚举名保持英文。
- **注释关键路径**：函数核心分支、平台特异性逻辑、生命周期清理、IPC 接口必须附带中文注释，说明“为什么这样做”。

### 2. 启动与更新规范

- 严禁在日常启动的主线程和启动 Splash 流程中硬编码未经配置控制的网络检查或循环重试。
- 启动更新必须遵循配置中的 `UpdateMethod`（`manual` / `background` / `startup`）：
  - 默认采用 `manual`（启动不更新，界面手动触发）；
  - `background` 必须在主界面就绪后在独立后台线程静默拉取；
  - 仅在环境缺失（如首次解压无 `.venv`）时才允许进行初始化依赖安装。

### 3. 错误处理与健壮性

- 统一使用 `anyhow::Result` 或细分 Error 枚举，严禁未受保护的 `.unwrap()` 或 `.expect()` 直接作用于网络请求、文件 IO 或子进程启动。
- 启动器外壳必须绝对稳定，任何非致命错误（网络失败、更新下载中断、依赖检查失败）应记录日志并通过 UI/原生通知提示，不得导致外壳直接 Crash/Panic。

### 4. 国际化（i18n）准则

- **严禁硬编码 UI 字符串**：所有面向用户的界面文本、托盘菜单、原生通知、对话框内容，必须同步维护在 `locales/` 下的 4 种语言文件中：
  - `locales/zh-CN.yml`（简体中文，基准语言）
  - `locales/zh-TW.yml`（繁体中文）
  - `locales/ja.yml`（日语）
  - `locales/en.yml`（英语）
- **添加新多语言字符串的标准流程**：
  1. 在 `locales/zh-CN.yml` 对应功能模块分组下添加 key 和中文文案；
  2. 同步在 `zh-TW.yml`、`ja.yml`、`en.yml` 添加对应翻译；
  3. 在 Rust 代码中通过 `t!("module.key")` 调用（需 `.to_string()` 转换为 `String`）；
  4. 带参数的文本使用 `t!("module.key", param = value)`，YAML 模板中采用 `%{param}` 占位符。
- **语言检测与回退逻辑**（`src/i18n.rs`）：
  - 优先级：命令行参数 `--lang` / `--locale` > 系统 locale（`sys-locale` crate） > 回退到 `en`；
  - 匹配映射：`zh*` → `zh-CN`，`zh-TW`/`zh-HK`/`zh-Hant` → `zh-TW`，`ja*` → `ja`，其余回退为 `en`。

### 5. 全平台兼容守卫与构建保障（强制支持 Windows / macOS / Linux）

启动器作为跨平台桌面外壳，必须原生支持 **Windows（x86_64 / arm64）、macOS（Intel / Apple Silicon）以及 Linux（x86_64）**。
CI 构建会并发执行三大平台的编译与打包，任何单平台编译失败或未消除的警告均会导致发版阻断。

必须严格遵守以下跨平台编码准则：

1. **平台逻辑隔离与严格守卫**：
   - 涉及系统特异性的能力（Windows WinRT / Registry / macOS ActivationPolicy / Unix Signals / Linux freedesktop Notification 等），必须使用 `#[cfg(...)]` 进行严格平台守卫。
   - 暴露的原生能力接口（如 `open_external`、`open_folder`、`show_notification`）必须同步实现 Windows、macOS 和 Linux 三端逻辑，严禁单平台留空或漏写。

2. **避免条件编译下的符号与依赖缺失**：
   - **慎防清理“死代码”误删跨平台代码**：在当前宿主平台（如 Windows）下开发时，macOS（如 `tauri::RunEvent::Reopen`）或 Linux 专属分支中的函数与变量在本地不会被语法分析器标注引用，在重构或删除死代码时**必须主动审视 macOS / Linux 分支**，严禁误删跨平台运行所依赖的恢复函数或闭包变量。
   - **按需条件导入（Conditional Imports）**：特定平台专属的 crate、宏或类型（如 Windows 下的 `windows_registry`、Linux 下的 `notify_rust`），必须加 `#[cfg(...)]` 条件导入；对于跨平台公共函数签名中使用的通用类型（如 `anyhow::Result`），必须保证在所有启用该函数的平台（如 `#[cfg(any(windows, target_os = "linux"))]`）均有导入。

3. **零告警编译保证（No Warnings）**：
   - 部分类型或变量若仅在特定平台被使用（例如系统托盘仅在 Windows/macOS 创建，Linux 不创建），在通用作用域或非目标平台上必须添加 `#[allow(unused_imports)]` 或 `#[allow(unused_variables)]`，确保在 Windows、macOS 和 Linux 上均可 0 警告通过编译。

### 6. 自定义标题栏与 DOM 注入规范

- 注入脚本位于 `main_window_titlebar_injection_script()`，使用纯原生 JavaScript 与 CSS；
- CSS 类名使用 `.alas-` 前缀隔离命名空间，避免与 WebUI 内部样式冲突；
- 前端与 Rust 通信统一走 Tauri IPC（`window.__TAURI__.core.invoke`）。

### 7. 瘦外壳（Thin Shell）与接口暴露规范

- **Rust 只暴露底层接口**：Rust 外壳仅作为原子的系统能力提供者（窗口控制、系统原生 Toast 通知、系统浏览器、文件资源管理器、更新执行等），严禁承载应用层业务逻辑（如调度判定、任务状态、业务通知决策等），更不得反向通过 SSE/长连接在后台轮询读取 WebUI 的业务数据流。
- **接口统一收拢在 `window.alasDesktop`**：所有暴露给 Web 前端的桌面原生 API，一律统一挂载在 `window.alasDesktop` 对象下（如 `window.alasDesktop.showNotification(title, content)`），保持清晰、规范的单命名空间（类似 Electron 的 `window.electronAPI` 规范），严禁暴露散落的全局变量。
- **Web 端做逻辑业务开发**：业务条件判断、通知触发时机、多环境自适应（有壳调用 `window.alasDesktop` 底层接口，无壳回退为 Web 界面 UI Toast）全部由 Web 端（Python / 前端 JS）自行处理与决策。

### 8. 启动器外壳多步交互更新规范（Multi-Step Update Flow）

- **职责边界严格解耦**：右上角仅负责启动器外壳（Launcher）自身的检查与更新，主工程/WebUI 更新由内部专属模块独立负责，绝对禁止双向耦合。
- **四步交互状态机**：
  1. **检查更新（`Checking`）**：用户触发后仅比对远端 Release Manifest 版本号，**严禁**自动静默下载抢跑带宽。若已是最新转为 `AlreadyLatest`，并在 3.5s 后静默重置为 `Idle`；
  2. **发现新版本与用户确认（`Available`）**：检测到新版本后进入待确认状态，徽标展示琥珀高亮药丸（如 `★ 发现新版 vX.Y.Z`），主动弹出确认框；若用户取消则保留徽标，随时支持点击再次弹出；
  3. **分块多线程下载与动态推流（`Updating`）**：用户确认后启动分块多线程下载，并通过事件/轮询向前端实时推流进度百分比（`SplashUpdate`），徽标以线性渐变背景实时渲染动态进度条与数字；
  4. **下载完成与确认重启（`ReadyToRestart`）**：载荷下载并通过 SHA-256 校验后徽标变绿，弹出重启确认框，用户确认后调用 `window_exit_application` 触发 helper 进程无缝替换并重启生效。

### 9. 版本号与 Git Tag 命名规范

- **Git Tag 推荐带 `v`**：发版标签统一采用带 `v` 前缀的标准 SemVer 格式（如 **`v2.1.27`**），符合 GitHub Release 与主流开源社区规范；
- **配置内版本号保持纯数字**：`Cargo.toml` 与 `tauri.conf.json` 中的版本号遵循严格 SemVer，必须保持纯数字（如 `2.1.27`），严禁写入字母 `v`；
- **CI 全自动兼容剥离**：CI 打包脚本自动剥离 Tag 的 `v` 前缀写入安装包与 `stable.json` 的 `"version"` 字段，同时下载链接保持与 Release Tag 路径一致。无论打 `v2.1.xx` 还是 `2.1.xx` 均能稳定构建。

---

## 版本发布流程与规范

### 1. 版本号升级准则（SemVer）

- **版本号格式**：遵循严格的语义化版本命名（`Major.Minor.Patch`）。
  - **Patch（补丁版本，如 `2.1.30` -> `2.1.31`）**：日常 Bug 修复、交互细节微调、性能小幅优化、内部代码重构，不改变已有功能行为；
  - **Minor（次版本，如 `2.1.30` -> `2.2.0`）**：新增向后兼容的桌面系统能力、新功能特性、重要体验升级；
  - **Major（主版本，如 `2.1.30` -> `3.0.0`）**：整体架构级重构、重大不兼容变更。
- **配置文件内版本号保持纯数字**：
  - 源码配置文件 `Cargo.toml`（`version = "X.Y.Z"`）与 `tauri.conf.json`（`"version": "X.Y.Z"`）中必须保持纯数字 SemVer，**严禁包含字母 `v`**；
  - 修改 `Cargo.toml` 后须运行 `cargo check` 自动同步更新 `Cargo.lock`。
- **Git Tag 统一带 `v` 前缀**：
  - 发版标签统一使用带 `v` 前缀的标准 SemVer 格式（如 **`v2.1.31`**），符合 GitHub Release 与主流开源社区规范；
  - CI 打包脚本（`.github/workflows/package.yml`）会自动剥离 Tag 的 `v` 前缀并同步写入各产物与清单中。

### 2. 发布前强制自检清单（Pre-release Checklist）

发版之前必须依次完成以下自我审查，确保发版质量：

1. **语法与类型检查**：运行 `cargo check`，必须通过且**零警告（0 Warnings）**；
2. **多平台编译验证**：运行 `cargo test --no-run`，确保测试套件与各平台条件编译顺利通过；
3. **多语言词条完整性**：检查本次修改涉及的用户可见文本，必须在 `locales/` 下全部 4 种语言（`zh-CN.yml`、`zh-TW.yml`、`ja.yml`、`en.yml`）中均已补齐，严禁漏翻或出现占位符缺失；
4. **跨平台条件编译守卫**：检查 Windows、macOS、Linux 下的 `#[cfg]` 分支代码，确认非当前宿主平台的专属代码未被误删或破坏；
5. **工作区清洁度**：运行 `git status` 确认没有未纳入管理的临时测试脚本、临时打包产物或无意义修改。

### 3. 标准发版操作流程（Release Pipeline）

完成开发与自检后，按照以下四步执行标准发版：

#### 步骤一：提升版本号并同步锁文件
修改 `Cargo.toml` 与 `tauri.conf.json` 中的版本号，随后执行 `cargo check` 刷新 `Cargo.lock`：
```bash
# 1. 编辑 Cargo.toml 中的 version = "X.Y.Z"
# 2. 编辑 tauri.conf.json 中的 "version": "X.Y.Z"
# 3. 运行检查以更新 Cargo.lock
cargo check
```

#### 步骤二：提交版本升级变更
使用标准的 Conventional Commits 格式提交版本提升改动：
```bash
git add Cargo.toml tauri.conf.json Cargo.lock
git commit -m "chore(release): bump version to X.Y.Z"
# 或者包含本次核心变更说明：
# git commit -m "Release X.Y.Z: 修复XX问题并优化XX逻辑"
```

#### 步骤三：创建带附注的 Git 发版标签
创建带 `v` 前缀的附注标签（Annotated Tag）：
```bash
git tag -a vX.Y.Z -m "Release vX.Y.Z: <简要版本说明与更新要点>"
```

#### 步骤四：推送主分支与标签触发 CI 打包
将本地提交与 Tag 推送至远程仓库：
```bash
git push origin master
git push origin vX.Y.Z
# 或一并推送所有本地标签：
# git push origin master --tags
```

---

## Git 提交规范

### 提交前分析

提交代码前，必须分析当前 git 工作区中所有未提交的修改（staged、unstaged、untracked），按以下原则组织提交：

1. **理解修改目的**：主动理解每个修改的真实目的，不要简单粗暴地一次性将全部文件盲目打包提交；
2. **合理聚合**：按功能目标 / 修复目的 / 重构范围 / 工程变更进行聚合；
3. **语义边界**：避免把无关修改混在同一个 commit 中，拆分出具有明确语义边界的 commits；
4. **区分变更类型**：
   - 多语言词条、模板配置 → 独立或随功能对应提交
   - 核心逻辑重构 / 新增功能 → 独立提交
   - 临时调试、无用文件 → 提交前务必清理干净

### 提交信息格式

使用 Conventional Commits 风格，中文撰写：

```text
<type>(<scope>): <描述为什么改>
```

Type 类型：
- `feat`: 新功能（如新增更新模式、添加标题栏按钮）
- `fix`: 修复 bug（如修复端口冲突、托盘事件不生效）
- `refactor`: 重构（如拆分模块、优化生命周期管理）
- `perf`: 性能优化（如缩短启动耗时、减少内存占用）
- `chore`: 工程维护、依赖升级、脚本变更
- `docs`: 文档、说明更新
- `test`: 单元测试相关
- `build`: 构建脚本、Cargo.toml 配置变更
- `ci`: GitHub Actions 工作流调整

---

## 代码审查原则（强制自我审查）

每次修改代码后，必须在交付前对照以下清单自我审查：

1. **完整性**：是否完整满足用户需求，逻辑有无断层；
2. **启动性能**：是否无意引入了启动阶段的阻塞 IO 或网络请求；
3. **无关修改**：是否有意外修改无关文件（顺手修改污染、无意义格式变动）；
4. **多语言一致性**：新加入的文案是否在 4 种语言模板中均已补齐；
5. **平台兼容（全平台支持）**：`#[cfg]` 守卫是否周全，在非目标平台（Windows/macOS/Linux）上是否存在未导入类型、误删方法或未引用的编译警告；
6. **资源管理**：是否存在子进程孤儿泄漏风险、WebView 窗口销毁是否及时；
7. **自动化验证**：是否已通过 `cargo check` 及相关测试验证。
