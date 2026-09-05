---
description:
alwaysApply: true
---

# AGENTS.md

> **重要：必须使用中文交流。** 所有回复、注释、文档、提交信息均使用简体中文。代码中的变量名、函数名、结构体名使用英文。

本文件为 AI Agent 在本仓库（`AzurNext-Launcher`）中工作时提供指导与核心约束规范。与 `CLAUDE.md` 互为补充——CLAUDE.md 偏完整架构速查，本文件偏工程规范、编码模式与实操要点。

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
cargo build                     # 构建 Debug 版本
cargo build --release           # 构建 Release 版本（启用 LTO 优化）
cargo tauri dev                 # 启动 Tauri 开发调试模式
```

> **注意**：Windows 下启动器内置了管理员权限 Manifest，非提权环境下直接执行生成的可执行文件会报 `os error 740`。日常自动化验证推荐运行 `cargo check` 与 `cargo test --no-run`。

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
  - `locales/zh-CN.yml`（简体中文，基准）
  - `locales/zh-TW.yml`（繁体中文）
  - `locales/en.yml`（英语）
  - `locales/ja.yml`（日语）
- 在 Rust 中统一通过 `t!("module.key")` 或 `t!("module.key", param = value)` 宏引用。

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
