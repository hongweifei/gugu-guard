//! 自启命令与 systemd unit 的纯构造函数：不碰系统，任意平台都可单测。

use std::path::Path;

/// Windows：任务计划 / Run 项 / 服务共用的启动命令。路径一律加引号，含空格也安全。
/// （unix 下用不到，故仅在 Windows 与测试构建里编译，避免 dead_code 在 CI 上变成错误。）
#[cfg(any(windows, test))]
pub(super) fn run_command(exe: &Path, config: &Path) -> String {
    format!("\"{}\" run -c \"{}\"", exe.display(), config.display())
}

/// Windows：用户级自启命令 —— 追加 `--detach`，登录时由自己转入后台而非弹控制台。
#[cfg(any(windows, test))]
pub(super) fn run_command_detached(exe: &Path, config: &Path) -> String {
    format!("{} --detach", run_command(exe, config))
}

/// systemd unit 文本。`ExecStart`/`WorkingDirectory` 一律加引号，含空格的安装路径可用。
#[cfg(any(unix, test))]
pub(super) fn build_systemd_unit(
    exe: &Path,
    config: &Path,
    workdir: &Path,
    display: &str,
    user_level: bool,
) -> String {
    let (after, wanted) = if user_level {
        ("default.target", "default.target")
    } else {
        ("network.target", "multi-user.target")
    };
    format!(
        "[Unit]\n\
         Description={display}\n\
         After={after}\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"{exe}\" run -c \"{config}\"\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         KillSignal=SIGTERM\n\
         TimeoutStopSec=30\n\
         WorkingDirectory=\"{workdir}\"\n\n\
         [Install]\n\
         WantedBy={wanted}\n",
        display = display,
        after = after,
        exe = exe.display(),
        config = config.display(),
        workdir = workdir.display(),
        wanted = wanted,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_quotes_paths_with_spaces() {
        let cmd = run_command(
            Path::new(r"C:\Program Files\gugu guard\gugu.exe"),
            Path::new(r"C:\my cfg\gugu.toml"),
        );
        assert_eq!(
            cmd,
            "\"C:\\Program Files\\gugu guard\\gugu.exe\" run -c \"C:\\my cfg\\gugu.toml\""
        );
        // 两侧都有引号：路径含空格时 schtasks /reg 才不会把参数拆开
        assert_eq!(cmd.matches('"').count(), 4);
    }

    #[test]
    fn detached_command_appends_flag_after_args() {
        let cmd = run_command_detached(Path::new("/opt/gugu"), Path::new("/opt/gugu.toml"));
        assert_eq!(cmd, "\"/opt/gugu\" run -c \"/opt/gugu.toml\" --detach");
    }

    #[test]
    fn systemd_unit_quotes_paths_and_targets_multi_user() {
        let unit = build_systemd_unit(
            Path::new("/opt/my gugu/gugu"),
            Path::new("/opt/my gugu/gugu.toml"),
            Path::new("/opt/my gugu"),
            "GuguGuard Test",
            false,
        );
        assert!(
            unit.contains("ExecStart=\"/opt/my gugu/gugu\" run -c \"/opt/my gugu/gugu.toml\""),
            "含空格路径必须被引号包住: {unit}"
        );
        assert!(unit.contains("WorkingDirectory=\"/opt/my gugu\""), "{unit}");
        assert!(unit.contains("WantedBy=multi-user.target"), "{unit}");
        assert!(unit.contains("After=network.target"), "{unit}");
        assert!(unit.contains("Restart=on-failure"), "{unit}");
    }

    #[test]
    fn systemd_user_unit_targets_default_target() {
        let unit = build_systemd_unit(
            Path::new("/home/u/gugu"),
            Path::new("/home/u/gugu.toml"),
            Path::new("/home/u"),
            "GuguGuard Test",
            true,
        );
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert!(unit.contains("After=default.target"), "{unit}");
        assert!(
            !unit.contains("multi-user.target"),
            "用户级不应挂系统 target: {unit}"
        );
    }

    #[test]
    fn systemd_unit_has_required_sections() {
        let unit = build_systemd_unit(
            Path::new("/x/gugu"),
            Path::new("/x/gugu.toml"),
            Path::new("/x"),
            "GuguGuard Test",
            false,
        );
        for section in ["[Unit]", "[Service]", "[Install]"] {
            assert!(unit.contains(section), "缺少 {section}: {unit}");
        }
        assert!(unit.ends_with('\n'), "单元文件应以换行结尾");
    }
}
