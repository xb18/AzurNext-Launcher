use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::notify::show_system_notification;
use crate::setup::{get_update_method, UpdateMethod};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum UpdateState {
    Idle,
    Checking,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub launcher_updated: Option<String>,
}

/// 触发更新流程（供手动点击或后台定时器调用）
pub fn trigger_update(app: AppHandle, is_background: bool) -> Result<String> {
    if IS_UPDATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("Update is already in progress; skipping duplicate trigger");
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
        let run_result = perform_update_pipeline(&app_handle, is_background);
        IS_UPDATING.store(false, Ordering::SeqCst);
        match run_result {
            Ok(outcome) => {
                if let Some(version) = outcome.launcher_updated {
                    emit_and_set_update_state(&app_handle, UpdateState::ready_to_restart(&version));
                    show_system_notification(
                        &app_handle,
                        &t!("notify.update_title"),
                        &t!("notify.update_success_restart"),
                    );
                } else {
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
            }
            Err(err) => {
                let err_msg = format!("{err:#}");
                warn!("Update task failed: {err_msg}");
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

    Ok("started".to_string())
}

fn perform_update_pipeline(app: &AppHandle, _is_background: bool) -> Result<UpdateOutcome> {
    info!("Starting launcher update check...");

    // 检查启动器外壳自身更新
    emit_and_set_update_state(app, UpdateState::Checking);
    let mut launcher_updated = None;
    match crate::check_and_download_launcher_update_payload(|update| {
        emit_and_set_update_state(
            app,
            UpdateState::updating(
                update.progress,
                update.title,
                update.detail,
            ),
        );
    }) {
        Ok(Some((version, payload_path))) => {
            info!("Downloaded new launcher update version {version} to {:?}", payload_path);
            set_pending_launcher_update(payload_path);
            launcher_updated = Some(version);
        }
        Ok(None) => {
            info!("Launcher executable is already up to date");
        }
        Err(err) => {
            warn!("Failed checking launcher update: {err:#}");
            return Err(err);
        }
    }

    Ok(UpdateOutcome {
        launcher_updated,
    })
}

/// 启动后台静默更新调度（当配置为 background 时）
pub fn start_background_updater_if_enabled(app: AppHandle) {
    if get_update_method() == UpdateMethod::Background {
        thread::spawn(move || {
            info!("Background updater scheduled; will check updates in 30 seconds");
            thread::sleep(Duration::from_secs(30));
            if let Err(e) = trigger_update(app, true) {
                warn!("Failed to run background update: {e:#}");
            }
        });
    }
}
