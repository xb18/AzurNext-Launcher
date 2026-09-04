use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tracing::{error, info, warn};

use crate::notify::show_system_notification;
use crate::setup::{get_update_method, run_repository_and_dependency_update, UpdateMethod};

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
    ReadyToRestart,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub launcher_updated: bool,
    pub repo_updated: bool,
}

/// 触发更新流程（供手动点击或后台定时器调用）
pub fn trigger_update(app: AppHandle, is_background: bool) -> Result<String> {
    if IS_UPDATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("Update is already in progress; skipping duplicate trigger");
        return Ok("in_progress".to_string());
    }

    set_update_state(UpdateState::Checking);

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
                if outcome.launcher_updated {
                    set_update_state(UpdateState::ReadyToRestart);
                    show_system_notification(
                        &app_handle,
                        &t!("notify.update_title"),
                        &t!("notify.update_success_restart"),
                    );
                } else if outcome.repo_updated {
                    set_update_state(UpdateState::AlreadyLatest);
                    show_system_notification(
                        &app_handle,
                        &t!("notify.update_title"),
                        &t!("notify.update_success"),
                    );
                    thread::sleep(Duration::from_secs(3));
                    set_update_state(UpdateState::Idle);
                } else {
                    set_update_state(UpdateState::AlreadyLatest);
                    if !is_background {
                        show_system_notification(
                            &app_handle,
                            &t!("notify.default_title"),
                            &t!("notify.update_already_latest"),
                        );
                    }
                    thread::sleep(Duration::from_secs(3));
                    set_update_state(UpdateState::Idle);
                }
            }
            Err(err) => {
                let err_msg = format!("{err:#}");
                warn!("Update task failed: {err_msg}");
                set_update_state(UpdateState::Failed {
                    detail: err_msg.clone(),
                });
                show_system_notification(
                    &app_handle,
                    &t!("notify.default_title"),
                    &t!("notify.update_failed", error = err_msg),
                );
            }
        }
    });

    Ok("started".to_string())
}

fn perform_update_pipeline(_app: &AppHandle, is_background: bool) -> Result<UpdateOutcome> {
    info!("Starting update pipeline (is_background={})...", is_background);

    // 阶段 1：检查启动器自身更新
    set_update_state(UpdateState::Checking);
    let mut launcher_updated = false;
    match crate::check_and_download_launcher_update_payload(|update| {
        set_update_state(UpdateState::updating(
            update.progress,
            update.title,
            update.detail,
        ));
    }) {
        Ok(Some((version, payload_path))) => {
            info!("Downloaded new launcher update version {version} to {:?}", payload_path);
            set_pending_launcher_update(payload_path);
            launcher_updated = true;
        }
        Ok(None) => {
            info!("Launcher executable is already up to date");
        }
        Err(err) => {
            warn!("Failed checking launcher update: {err:#}");
            // 启动器更新检查失败不阻塞仓库代码更新
        }
    }

    // 阶段 2：检查并拉取 AzurNext 仓库代码与 uv 依赖
    set_update_state(UpdateState::updating(
        0,
        t!("setup.updating"),
        t!("setup.fetching_patches"),
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let repo_updated = match run_repository_and_dependency_update(cancel, |update| {
        set_update_state(UpdateState::updating(
            update.progress,
            update.title,
            update.detail,
        ));
    }) {
        Ok(updated) => updated,
        Err(err) => {
            error!("Repository update failed: {err:#}");
            return Err(err);
        }
    };

    Ok(UpdateOutcome {
        launcher_updated,
        repo_updated,
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
