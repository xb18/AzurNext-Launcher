//! 桌面通知模块：按平台提供原生系统通知的底层能力。
//! 分发层按平台选择通知后端——Windows 用 tauri-winrt-notification（Toast），
//! Linux 用 notify-rust，macOS 等其余平台用 tauri-plugin-notification。

use std::sync::Arc;

#[cfg(windows)]
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use anyhow::anyhow;
#[cfg(any(windows, target_os = "linux"))]
use anyhow::Result;

#[cfg(target_os = "linux")]
use notify_rust::{Hint, Notification};
use rust_i18n::t;
#[cfg(all(not(windows), not(target_os = "linux")))]
use tauri_plugin_notification::NotificationExt;
use tracing::warn;

/// 通知被点击时执行的回调。
///
/// 启动器用它把通知点击统一汇聚到"唤起主窗口"等逻辑（由 main.rs 提供），
/// 以 `Arc` 包装便于克隆给各平台的通知实现与后台线程。
pub type NotificationClickHandler = Arc<dyn Fn() + Send + Sync + 'static>;

/// Windows Toast 通知的 AUMID（AppUserModelId）。
#[cfg(windows)]
const WINDOWS_APP_ID: &str = "moe.taiho.azurnext-launcher.notification";

/// 通知在操作中心里显示的应用名（随界面语言本地化）。
#[cfg(windows)]
fn windows_app_name() -> String {
    t!("notify.info_app_name").to_string()
}

/// 编译期嵌入的应用图标字节，供 Windows Toast 图标落盘使用。
///
/// Toast 的 `IconUri` 必须指向磁盘上的真实文件，无法直接内嵌内存数据，
/// 因此携带一份图标内容，运行时写入本地数据目录（见
/// [`ensure_windows_notification_icon`]）。
#[cfg(windows)]
const WINDOWS_NOTIFICATION_ICON: &[u8] = include_bytes!("../icons/icon.png");

/// 弹出系统原生通知（底层接口）
pub fn show_notification(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    on_click: &NotificationClickHandler,
) {
    let title = title.trim();
    let default_title = t!("notify.default_title");
    let display_title = if title.is_empty() {
        default_title.as_ref()
    } else {
        title
    };
    let display_body = body.trim();

    // Windows 走独立路径：Toast 支持挂点击回调。
    #[cfg(windows)]
    {
        if let Err(e) = show_windows_notification(display_title, display_body, on_click.clone()) {
            warn!("Failed to show Windows notification: {e}");
        }
        // 该平台不使用 Tauri 通知插件，显式消费参数避免未使用告警。
        let _ = app;
    }

    // Linux 走 freedesktop 通知规范（notify-rust），通过 "default" 动作实现点击回调。
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = show_linux_notification(display_title, display_body, on_click.clone()) {
            warn!("Failed to show Linux notification: {e}");
        }
        // 同上，此平台不消费 Tauri 通知插件。
        let _ = app;
    }

    // macOS 等其余平台使用 Tauri 官方通知插件；该插件不暴露点击回调，
    // 故忽略 on_click（点击行为由系统按应用图标处理）。
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        if let Err(e) = app.notification().builder().title(display_title).body(display_body).show() {
            warn!("Failed to show system notification: {e}");
        }
        let _ = on_click;
    }
}

/// 供启动器内部直接使用的通知函数（如更新成功提示）
#[allow(dead_code)]
pub fn show_system_notification(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
) {
    let handler: NotificationClickHandler = Arc::new(|| {});
    show_notification(app, title, body, &handler);
}

/// 在 Windows 上弹出一条 Toast 通知。
///
/// 点击 Toast 会触发启动器统一的 `on_click` 回调。
///
/// # Errors
///
/// AUMID 注册表写入失败、图标落盘失败或 Toast 展示失败（例如系统
/// 通知功能被禁用）时返回错误。
#[cfg(windows)]
fn show_windows_notification(
    title: &str,
    body: &str,
    on_click: NotificationClickHandler,
) -> Result<()> {
    let app_id = WINDOWS_APP_ID;
    let app_name = windows_app_name();

    // Toast 的 IconUri 要求磁盘路径且以 URI 形式书写，因此先把嵌入图标
    // 落盘，再把路径分隔符统一为正斜杠，避免反斜杠带来的转义歧义。
    let icon_path = ensure_windows_app_user_model_id(app_id, &app_name)?;
    let icon_uri_path = icon_path.to_string_lossy().replace('\\', "/");
    tauri_winrt_notification::Toast::new(app_id)
        .icon(
            Path::new(&icon_uri_path),
            tauri_winrt_notification::IconCrop::Square,
            &app_name,
        )
        .title(title)
        .text1(body)
        // Short：通知短暂显示后自动收进操作中心，减少打扰。
        .duration(tauri_winrt_notification::Duration::Short)
        // 用户点击 Toast 时触发，转发给启动器统一的点击处理。
        .on_activated(move |_| {
            on_click();
            Ok(())
        })
        .show()
        .map_err(|e| anyhow!("{e:?}"))
}

/// 确保指定 AUMID 已注册到当前用户注册表，并返回可用的图标路径。
///
/// Toast 通知要求 AUMID 预先注册（`HKCU\SOFTWARE\Classes\AppUserModelId`），
/// 否则系统无法在通知中显示应用名称与图标。写入幂等：重复调用只做覆盖。
///
/// # Errors
///
/// 注册表创建/写入失败，或通知图标落盘失败时返回错误。
#[cfg(windows)]
fn ensure_windows_app_user_model_id(id: &str, name: &str) -> Result<PathBuf> {
    // 图标路径同时用于注册表 IconUri 与 Toast 的 icon 参数，先准备妥当。
    let icon_path = ensure_windows_notification_icon()?;
    let key = windows_registry::CURRENT_USER
        .create(format!(r"SOFTWARE\Classes\AppUserModelId\{id}"))
        .map_err(|e| anyhow!("{e:?}"))?;

    // DisplayName、IconBackgroundColor 与 IconUri 共同决定操作中心里的
    // 显示名称与圆形图标外观。
    key.set_string("DisplayName", name)
        .map_err(|e| anyhow!("{e:?}"))?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|e| anyhow!("{e:?}"))?;
    key.set_hstring("IconUri", &icon_path.as_path().into())
        .map_err(|e| anyhow!("{e:?}"))?;
    Ok(icon_path)
}

/// 把编译期嵌入的通知图标落盘到本地数据目录，返回其路径。
///
/// Toast 的 IconUri 只认磁盘文件，因此需要持久化一份图标；当嵌入图标
/// 内容发生变化（随升级更换）时覆盖重写，保证注册表里的路径始终有效。
///
/// # Errors
///
/// 本地数据目录不存在且无法创建，或图标读写失败时返回错误。
#[cfg(windows)]
fn ensure_windows_notification_icon() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!(t!("errors.appdata_not_found")))?
        .join("AzurNextLauncher");
    fs::create_dir_all(&data_dir)?;

    // 已存在的图标内容与嵌入版本一致时跳过写入，减少磁盘 IO。
    let icon_path = data_dir.join("notification-icon.png");
    let should_write = fs::read(&icon_path)
        .map(|current| current != WINDOWS_NOTIFICATION_ICON)
        .unwrap_or(true);
    if should_write {
        fs::write(&icon_path, WINDOWS_NOTIFICATION_ICON)?;
    }

    Ok(icon_path)
}

/// 在 Linux 上通过 freedesktop 通知规范弹出通知。
///
/// 使用 notify-rust 直接对接通知守护进程；"default" 动作即用户点击
/// 通知本体，触发后转发给启动器统一的点击回调。
///
/// # Errors
///
/// 通知守护进程不可用或展示失败时返回错误。
#[cfg(target_os = "linux")]
fn show_linux_notification(
    title: &str,
    body: &str,
    on_click: NotificationClickHandler,
) -> Result<()> {
    let mut notification = Notification::new();
    notification
        .summary(title)
        .body(body)
        // auto_icon 让通知守护进程按应用自动匹配图标，无需自带图标文件。
        .auto_icon()
        .action("default", &t!("notify.open").to_string())
        // Resident 提示通知常驻、不因超时自动消失，确保重要事件被看到。
        .hint(Hint::Resident(true));
    let handle = notification.show()?;

    // wait_for_action 会阻塞等待用户交互，必须放入独立线程。
    std::thread::spawn(move || {
        handle.wait_for_action(move |action| {
            if action == "default" {
                on_click();
            }
        });
    });

    Ok(())
}
