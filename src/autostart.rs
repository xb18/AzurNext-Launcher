//! 开机自启模块：查询与设置启动器随系统自动启动的状态。
//!
//! 仅 Windows 提供完整实现——通过写入当前用户注册表的
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 键实现开机自启，
//! 并附加 `--start-minimized` 参数使启动后直接最小化到托盘。
//! 其他平台返回"不支持"状态，调用方据此在界面上隐藏相关选项。

use anyhow::{anyhow, Result};
use serde::Serialize;

/// 开机自启状态的查询结果。
///
/// 序列化后经 Tauri 命令返回给前端展示。
#[derive(Debug, Serialize)]
pub struct AutostartStatus {
    /// 当前自启项是否指向本启动器（即自启已生效）。
    pub enabled: bool,
    /// 当前平台是否支持开机自启（仅 Windows 为 true）。
    pub supported: bool,
    /// 注册表 Run 键中当前记录的原始命令行，未设置时为 `None`。
    pub value: Option<String>,
}

/// 查询当前平台的开机自启状态。
///
/// # Errors
/// Windows 上读取计划任务失败时返回 Err；
/// 非 Windows 平台始终返回"不支持"的固定状态。
pub fn query() -> Result<AutostartStatus> {
    query_platform()
}

/// 启用或禁用开机自启，并返回设置后的最新状态。
///
/// # Errors
/// Windows 上创建或删除计划任务失败时返回 Err；
/// 非 Windows 平台一律返回 Err（不支持该功能）。
pub fn set_enabled(enabled: bool) -> Result<AutostartStatus> {
    set_enabled_platform(enabled)
}

/// Windows 实现：通过计划任务管理开机自启（HighestAvailable 权限避免 UAC 开机拦截）。
#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::{
    env,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Command,
};
#[cfg(windows)]
use tracing::{info, warn};

#[cfg(windows)]
const TASK_NAME: &str = "AzurNext";

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(windows, test))]
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

/// Windows 实现：按需创建或删除计划任务，然后回查最新状态。
#[cfg(windows)]
fn set_enabled_platform(enabled: bool) -> Result<AutostartStatus> {
    cleanup_legacy_run_value();

    let schtasks = get_schtasks_path();

    if enabled {
        let exe = env::current_exe()?;
        let working_dir = exe.parent().unwrap_or_else(|| Path::new("."));
        let xml_content = build_task_xml(&exe, working_dir);

        let temp_dir = tempfile::Builder::new()
            .prefix("azurnext_task_")
            .tempdir()?;
        let temp_path = temp_dir.path().join("task.xml");
        let utf16: Vec<u16> = std::iter::once(0xFEFF)
            .chain(xml_content.encode_utf16())
            .collect();
        let bytes: Vec<u8> = utf16
            .into_iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        std::fs::write(&temp_path, bytes)?;

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

    // 统一走查询路径返回，确保调用方拿到的就是计划任务的真实落盘结果
    query_platform()
}

/// 归一化路径字符串用于比较：去首尾空白、统一斜杠方向、转小写。
#[cfg(windows)]
fn normalize_path(value: &str) -> String {
    value.trim().replace('/', "\\").to_ascii_lowercase()
}

/// 非 Windows 平台：固定返回"不支持"状态，便于前端隐藏自启开关。
#[cfg(not(windows))]
fn query_platform() -> Result<AutostartStatus> {
    Ok(AutostartStatus {
        enabled: false,
        supported: false,
        value: None,
    })
}

/// 非 Windows 平台：不支持设置开机自启，直接返回错误。
#[cfg(not(windows))]
fn set_enabled_platform(_enabled: bool) -> Result<AutostartStatus> {
    Err(anyhow!("Autostart is only supported on Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a & b < c > d \" e ' f"), "a &amp; b &lt; c &gt; d &quot; e &apos; f");
    }

    #[test]
    fn test_build_task_xml() {
        let exe = Path::new(r"C:\Program Files (x86)\AzurNext\azurnext.exe");
        let dir = Path::new(r"C:\Program Files (x86)\AzurNext");
        let xml = build_task_xml(exe, dir);
        assert!(xml.contains(r"C:\Program Files (x86)\AzurNext\azurnext.exe"));
        assert!(xml.contains("--start-minimized"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
    }

    #[test]
    fn test_temp_xml_write_and_read() -> Result<()> {
        let temp_dir = tempfile::Builder::new()
            .prefix("azurnext_task_test_")
            .tempdir()?;
        let temp_path = temp_dir.path().join("task.xml");
        let sample = "<Task>test</Task>";
        let utf16: Vec<u16> = std::iter::once(0xFEFF)
            .chain(sample.encode_utf16())
            .collect();
        let bytes: Vec<u8> = utf16.into_iter().flat_map(|u| u.to_le_bytes()).collect();
        std::fs::write(&temp_path, &bytes)?;

        let read_bytes = std::fs::read(&temp_path)?;
        assert_eq!(read_bytes, bytes);
        Ok(())
    }
}

