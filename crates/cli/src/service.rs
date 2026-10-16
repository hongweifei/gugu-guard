//! 服务标识常量：Windows 服务名/显示名、systemd 单元名，
//! 供 install（任务计划/systemd unit）与 windows_service 共用，单一来源。

/// 显示名（Windows 服务与 systemd unit 的 Description 同源）。
pub(crate) const SERVICE_DISPLAY: &str = "咕咕鸽进程守护 (GuguGuard)";

/// Windows：Windows Service 名与任务计划程序任务名。
#[cfg(windows)]
pub(crate) const SERVICE_NAME: &str = "GuguGuard";

/// Linux：systemd 单元名与文件路径。
#[cfg(unix)]
pub(crate) const SYSTEMD_UNIT: &str = "guguguard";
#[cfg(unix)]
pub(crate) const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/guguguard.service";
