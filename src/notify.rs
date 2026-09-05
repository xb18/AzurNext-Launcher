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

pub type NotificationClickHandler = Arc<dyn Fn() + Send + Sync + 'static>;

#[cfg(windows)]
const WINDOWS_APP_ID: &str = "moe.taiho.azurnext-launcher.notification";

fn windows_app_name() -> String {
    t!("notify.info_app_name").to_string()
}

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

    #[cfg(windows)]
    {
        if let Err(e) = show_windows_notification(display_title, display_body, on_click.clone()) {
            warn!("Failed to show Windows notification: {e}");
        }
        let _ = app;
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = show_linux_notification(display_title, display_body, on_click.clone()) {
            warn!("Failed to show Linux notification: {e}");
        }
        let _ = app;
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        if let Err(e) = app.notification().builder().title(display_title).body(display_body).show() {
            warn!("Failed to show system notification: {e}");
        }
        let _ = on_click;
    }
}

/// 供启动器内部直接使用的通知函数（如更新成功提示）
pub fn show_system_notification(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
) {
    let handler: NotificationClickHandler = Arc::new(|| {});
    show_notification(app, title, body, &handler);
}

#[cfg(windows)]
fn show_windows_notification(
    title: &str,
    body: &str,
    on_click: NotificationClickHandler,
) -> Result<()> {
    let app_id = WINDOWS_APP_ID;
    let app_name = windows_app_name();

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
        .duration(tauri_winrt_notification::Duration::Short)
        .on_activated(move |_| {
            on_click();
            Ok(())
        })
        .show()
        .map_err(|e| anyhow!("{e:?}"))
}

#[cfg(windows)]
fn ensure_windows_app_user_model_id(id: &str, name: &str) -> Result<PathBuf> {
    let icon_path = ensure_windows_notification_icon()?;
    let key = windows_registry::CURRENT_USER
        .create(format!(r"SOFTWARE\Classes\AppUserModelId\{id}"))
        .map_err(|e| anyhow!("{e:?}"))?;

    key.set_string("DisplayName", name)
        .map_err(|e| anyhow!("{e:?}"))?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|e| anyhow!("{e:?}"))?;
    key.set_hstring("IconUri", &icon_path.as_path().into())
        .map_err(|e| anyhow!("{e:?}"))?;
    Ok(icon_path)
}

#[cfg(windows)]
fn ensure_windows_notification_icon() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("Unable to resolve local app data directory"))?
        .join("AzurNextLauncher");
    fs::create_dir_all(&data_dir)?;

    let icon_path = data_dir.join("notification-icon.png");
    let should_write = fs::read(&icon_path)
        .map(|current| current != WINDOWS_NOTIFICATION_ICON)
        .unwrap_or(true);
    if should_write {
        fs::write(&icon_path, WINDOWS_NOTIFICATION_ICON)?;
    }

    Ok(icon_path)
}

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
        .auto_icon()
        .action("default", &t!("notify.open").to_string())
        .hint(Hint::Resident(true));
    let handle = notification.show()?;

    std::thread::spawn(move || {
        handle.wait_for_action(move |action| {
            if action == "default" {
                on_click();
            }
        });
    });

    Ok(())
}
