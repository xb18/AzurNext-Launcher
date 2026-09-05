use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::{anyhow, Result};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::notify::show_system_notification;
use crate::setup::{get_update_method, UpdateMethod};
use crate::LauncherUpdatePlatform;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum UpdateState {
    Idle,
    Checking,
    Available {
        #[serde(default)]
        version: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        detail: String,
    },
    Updating {
        #[serde(default)]
        progress: u8,
        #[serde(default)]
        title: String,
        #[serde(default)]
        detail: String,
    },
    ReadyToRestart {
        #[serde(default)]
        version: String,
    },
    AlreadyLatest,
    Failed {
        detail: String,
    },
}

impl UpdateState {
    pub fn updating(progress: u8, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Updating {
            progress,
            title: title.into(),
            detail: detail.into(),
        }
    }

    pub fn ready_to_restart(version: impl Into<String>) -> Self {
        Self::ReadyToRestart {
            version: version.into(),
        }
    }
}

static IS_UPDATING: AtomicBool = AtomicBool::new(false);
static PENDING_LAUNCHER_UPDATE: Mutex<Option<PathBuf>> = Mutex::new(None);
static PENDING_UPDATE_PLATFORM: Mutex<Option<(String, LauncherUpdatePlatform)>> = Mutex::new(None);
static CURRENT_STATE: Mutex<UpdateState> = Mutex::new(UpdateState::Idle);

pub fn get_update_state() -> UpdateState {
    CURRENT_STATE.lock().unwrap().clone()
}

pub fn set_update_state(state: UpdateState) {
    let mut lock = CURRENT_STATE.lock().unwrap();
    *lock = state;
}

pub fn emit_and_set_update_state(app: &AppHandle, state: UpdateState) {
    set_update_state(state.clone());
    if let Err(e) = app.emit("update-status-changed", &state) {
        warn!("Failed to emit update status event: {e:#}");
    }
}

#[allow(dead_code)]
pub fn is_update_in_progress() -> bool {
    IS_UPDATING.load(Ordering::SeqCst)
}

pub fn set_pending_launcher_update(path: PathBuf) {
    let mut lock = PENDING_LAUNCHER_UPDATE.lock().unwrap();
    *lock = Some(path);
}

pub fn take_pending_launcher_update() -> Option<PathBuf> {
    let mut lock = PENDING_LAUNCHER_UPDATE.lock().unwrap();
    lock.take()
}

#[allow(dead_code)]
pub fn has_pending_launcher_update() -> bool {
    PENDING_LAUNCHER_UPDATE.lock().unwrap().is_some()
}

pub fn set_pending_update_platform(version: String, platform: LauncherUpdatePlatform) {
    let mut lock = PENDING_UPDATE_PLATFORM.lock().unwrap();
    *lock = Some((version, platform));
}

pub fn get_pending_update_platform() -> Option<(String, LauncherUpdatePlatform)> {
    let lock = PENDING_UPDATE_PLATFORM.lock().unwrap();
    lock.clone()
}

pub fn clear_pending_update_platform() {
    let mut lock = PENDING_UPDATE_PLATFORM.lock().unwrap();
    *lock = None;
}

/// 检查启动器外壳更新（第 1 步：仅比对版本，不自动下载）
pub fn check_launcher_update(app: AppHandle, is_background: bool) -> Result<String> {
    if IS_UPDATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("Update check/download is already in progress; skipping duplicate trigger");
        return Ok("in_progress".to_string());
    }

    emit_and_set_update_state(&app, UpdateState::Checking);

    if !is_background {
        show_system_notification(
            &app,
            &t!("notify.check_update_title"),
            &t!("notify.update_checking"),
        );
    }

    let app_handle = app.clone();
    thread::spawn(move || {
        let check_result = crate::check_launcher_update_available();
        IS_UPDATING.store(false, Ordering::SeqCst);
        match check_result {
            Ok(Some((version, platform))) => {
                info!("Found launcher update version {version}");
                set_pending_update_platform(version.clone(), platform);
                emit_and_set_update_state(
                    &app_handle,
                    UpdateState::Available {
                        version: version.clone(),
                        title: t!("launcher_update.available_title").to_string(),
                        detail: t!("launcher_update.available_detail", version = version.clone()).to_string(),
                    },
                );
                show_system_notification(
                    &app_handle,
                    &t!("notify.update_title"),
                    &t!("notify.update_available", version = version),
                );
            }
            Ok(None) => {
                info!("Launcher executable is already up to date");
                clear_pending_update_platform();
                emit_and_set_update_state(&app_handle, UpdateState::AlreadyLatest);
                if !is_background {
                    show_system_notification(
                        &app_handle,
                        &t!("notify.default_title"),
                        &t!("notify.update_already_latest"),
                    );
                }
                thread::sleep(Duration::from_secs(3));
                emit_and_set_update_state(&app_handle, UpdateState::Idle);
            }
            Err(err) => {
                let err_msg = format!("{err:#}");
                warn!("Launcher update check failed: {err_msg}");
                clear_pending_update_platform();
                emit_and_set_update_state(&app_handle, UpdateState::Failed {
                    detail: err_msg.clone(),
                });
                show_system_notification(
                    &app_handle,
                    &t!("notify.default_title"),
                    &t!("notify.update_failed", error = err_msg),
                );
                thread::sleep(Duration::from_secs(5));
                emit_and_set_update_state(&app_handle, UpdateState::Idle);
            }
        }
    });

    Ok("checking_started".to_string())
}

/// 开始下载启动器更新（第 2 步：用户确认后调用）
pub fn start_download_launcher_update(app: AppHandle) -> Result<String> {
    let pending = get_pending_update_platform();
    let (version, platform) = match pending {
        Some(p) => p,
        None => {
            return Err(anyhow!("No pending update available to download"));
        }
    };

    if IS_UPDATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("Update download is already in progress");
        return Ok("in_progress".to_string());
    }

    emit_and_set_update_state(
        &app,
        UpdateState::updating(
            8,
            t!("launcher_update.updating"),
            t!("launcher_update.available_detail", version = version.clone()),
        ),
    );

    let app_handle = app.clone();
    thread::spawn(move || {
        let download_result = crate::download_launcher_update_payload(&platform, |update| {
            emit_and_set_update_state(
                &app_handle,
                UpdateState::updating(update.progress, update.title, update.detail),
            );
        });

        IS_UPDATING.store(false, Ordering::SeqCst);
        match download_result {
            Ok(payload_path) => {
                info!("Downloaded new launcher update version {version} to {:?}", payload_path);
                set_pending_launcher_update(payload_path);
                clear_pending_update_platform();
                emit_and_set_update_state(&app_handle, UpdateState::ready_to_restart(&version));
                show_system_notification(
                    &app_handle,
                    &t!("notify.update_title"),
                    &t!("notify.update_success_restart"),
                );
            }
            Err(err) => {
                let err_msg = format!("{err:#}");
                warn!("Launcher update download failed: {err_msg}");
                emit_and_set_update_state(&app_handle, UpdateState::Failed {
                    detail: err_msg.clone(),
                });
                show_system_notification(
                    &app_handle,
                    &t!("notify.default_title"),
                    &t!("notify.update_failed", error = err_msg),
                );
                thread::sleep(Duration::from_secs(5));
                emit_and_set_update_state(&app_handle, UpdateState::Idle);
            }
        }
    });

    Ok("download_started".to_string())
}

/// 取消或重置可用更新提示
pub fn cancel_or_dismiss_update(app: &AppHandle) {
    clear_pending_update_platform();
    emit_and_set_update_state(app, UpdateState::Idle);
}

/// 兼容老接口：触发检查更新流程
pub fn trigger_update(app: AppHandle, is_background: bool) -> Result<String> {
    check_launcher_update(app, is_background)
}

/// 启动后台静默更新调度（当配置为 background 时）
pub fn start_background_updater_if_enabled(app: AppHandle) {
    if get_update_method() == UpdateMethod::Background {
        thread::spawn(move || {
            info!("Background updater scheduled; will check updates in 30 seconds");
            thread::sleep(Duration::from_secs(30));
            if let Err(e) = check_launcher_update(app, true) {
                warn!("Failed to run background update: {e:#}");
            }
        });
    }
}

