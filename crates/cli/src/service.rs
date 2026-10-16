//! 服务标识常量：Windows 服务名/显示名、任务计划任务名、用户级 Run 键路径、
//! systemd 单元名（系统级与用户级），供 install 与 windows_service 共用，单一来源。

/// 显示名（Windows 服务与 systemd unit 的 Description 同源）。
pub(crate) const SERVICE_DISPLAY: &str = "咕咕鸽进程守护 (GuguGuard)";

/// Windows：Windows Service 名、任务计划程序任务名、Run 项值名。
#[cfg(windows)]
pub(crate) const SERVICE_NAME: &str = "GuguGuard";

/// Windows：当前用户登录自启的注册表位置（HKCU，无需管理员）。
/// 注意必须带 hive 根 —— `reg.exe add/query/delete` 要求完整键名。
#[cfg(windows)]
pub(crate) const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Linux：systemd 单元名（系统级与用户级同名）。
#[cfg(unix)]
pub(crate) const SYSTEMD_UNIT: &str = "guguguard";

/// Linux：系统级单元文件路径。
#[cfg(unix)]
pub(crate) const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/guguguard.service";

/// Linux：用户级单元文件相对 $HOME 的路径。
#[cfg(unix)]
pub(crate) const SYSTEMD_USER_UNIT_REL: &str = ".config/systemd/user/guguguard.service";
