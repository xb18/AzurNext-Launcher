use anyhow::{anyhow, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub supported: bool,
    pub value: Option<String>,
}

pub fn query() -> Result<AutostartStatus> {
    query_platform()
}

pub fn set_enabled(enabled: bool) -> Result<AutostartStatus> {
    set_enabled_platform(enabled)
}

#[cfg(windows)]
use std::{
    env,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
#[cfg(windows)]
use tracing::{info, warn};

#[cfg(windows)]
const TASK_NAME: &str = "AzurNext";

#[cfg(windows)]
const START_MINIMIZED_ARG: &str = "--start-minimized";

#[cfg(windows)]
const LEGACY_RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
const LEGACY_RUN_VALUE_NAME: &str = "AzurNext";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
fn get_schtasks_path() -> PathBuf {
    if let Ok(system_root) = env::var("SystemRoot") {
        let path = PathBuf::from(system_root).join("System32").join("schtasks.exe");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from("schtasks.exe")
}

/// 清理历史遗留的注册表 Run 键，避免新旧双重自启动或 UAC 拦截残留
#[cfg(windows)]
fn cleanup_legacy_run_value() {
    if let Ok(key) = windows_registry::CURRENT_USER.open(LEGACY_RUN_KEY_PATH) {
        let _ = key.remove_value(LEGACY_RUN_VALUE_NAME);
    }
}

#[cfg(windows)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn build_task_xml(exe_path: &Path, working_dir: &Path) -> String {
    let exe_str = xml_escape(&exe_path.to_string_lossy());
    let dir_str = xml_escape(&working_dir.to_string_lossy());
    let args_str = xml_escape(START_MINIMIZED_ARG);

    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>AzurNext Launcher AutoStart</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe_str}</Command>
      <Arguments>{args_str}</Arguments>
      <WorkingDirectory>{dir_str}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#
    )
}

#[cfg(windows)]
fn query_platform() -> Result<AutostartStatus> {
    let schtasks = get_schtasks_path();
    let output = match Command::new(&schtasks)
        .args(["/Query", "/TN", TASK_NAME, "/XML"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => out,
        Err(_) => {
            return Ok(AutostartStatus {
                enabled: false,
                supported: true,
                value: None,
            })
        }
    };

    if !output.status.success() {
        return Ok(AutostartStatus {
            enabled: false,
            supported: true,
            value: None,
        });
    }

    let xml_raw = output.stdout;
    let xml_text = if xml_raw.len() >= 2 && xml_raw[0] == 0xFF && xml_raw[1] == 0xFE {
        let u16_slice: Vec<u16> = xml_raw[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&u16_slice)
    } else {
        String::from_utf8_lossy(&xml_raw).into_owned()
    };

    let current_exe = env::current_exe().ok();
    let is_current = if let Some(ref exe) = current_exe {
        let norm_current = normalize_path(&exe.to_string_lossy());
        let norm_xml = normalize_path(&xml_text);
        norm_xml.contains(&norm_current)
    } else {
        true
    };

    let command_value = current_exe.map(|exe| {
        format!(r#""{}" {}"#, exe.to_string_lossy(), START_MINIMIZED_ARG)
    });

    Ok(AutostartStatus {
        enabled: is_current,
        supported: true,
        value: if is_current { command_value } else { None },
    })
}

#[cfg(windows)]
fn set_enabled_platform(enabled: bool) -> Result<AutostartStatus> {
    cleanup_legacy_run_value();

    let schtasks = get_schtasks_path();

    if enabled {
        let exe = env::current_exe()?;
        let working_dir = exe.parent().unwrap_or_else(|| Path::new("."));
        let xml_content = build_task_xml(&exe, working_dir);

        let temp_xml = tempfile::Builder::new()
            .prefix("azurnext_task_")
            .suffix(".xml")
            .tempfile()?;
        let temp_path = temp_xml.path();
        let utf16: Vec<u16> = std::iter::once(0xFEFF)
            .chain(xml_content.encode_utf16())
            .collect();
        let bytes: Vec<u8> = utf16
            .into_iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        std::fs::write(temp_path, bytes)?;

        let output = Command::new(&schtasks)
            .args(["/Create", "/TN", TASK_NAME, "/XML", &temp_path.to_string_lossy(), "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| anyhow!("Failed to execute schtasks: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let out = String::from_utf8_lossy(&output.stdout);
            let detail = if !err.trim().is_empty() {
                err.trim()
            } else {
                out.trim()
            };
            return Err(anyhow!("Failed to create scheduled task: {detail}"));
        }
        info!("Successfully created autostart scheduled task '{TASK_NAME}' with HighestAvailable privileges");
    } else {
        let output = Command::new(&schtasks)
            .args(["/Delete", "/TN", TASK_NAME, "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| anyhow!("Failed to execute schtasks: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.contains("找不到") && !err.contains("cannot find") {
                warn!("Failed to delete scheduled task '{TASK_NAME}': {err}");
            }
        } else {
            info!("Successfully removed autostart scheduled task '{TASK_NAME}'");
        }
    }

    query_platform()
}

#[cfg(windows)]
fn normalize_path(value: &str) -> String {
    value.trim().replace('/', "\\").to_ascii_lowercase()
}

#[cfg(not(windows))]
fn query_platform() -> Result<AutostartStatus> {
    Ok(AutostartStatus {
        enabled: false,
        supported: false,
        value: None,
    })
}

#[cfg(not(windows))]
fn set_enabled_platform(_enabled: bool) -> Result<AutostartStatus> {
    Err(anyhow!("Autostart is only supported on Windows"))
}
