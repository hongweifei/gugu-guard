//! 停止与强杀：自定义停止命令、默认信号、超时兜底。

use tokio::process::Child;

#[cfg(windows)]
use super::command::CREATE_NO_WINDOW;

pub(super) async fn run_stop_command(name: &str, stop_cmd: &str, working_dir: Option<&std::path::Path>, env: &std::collections::HashMap<String, String>) {
    let mut cmd;
    #[cfg(windows)]
    {
        cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(stop_cmd);
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(stop_cmd);
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    cmd.envs(env)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    tracing::info!("[{}] 执行自定义停止命令: {}", name, stop_cmd);
    match cmd.status().await {
        Ok(s) => tracing::debug!("[{}] 停止命令退出码: {:?}", name, s.code()),
        Err(e) => tracing::warn!("[{}] 停止命令执行失败: {}", name, e),
    }
}

pub(super) async fn send_default_stop_signal(pid: Option<u32>) {
    if let Some(pid) = pid {
        #[cfg(windows)]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }

        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }
}

pub(super) async fn force_kill(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        // force_kill 是兜底，直接用 taskkill /F 确保进程终止
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &child.id().unwrap_or_default().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}
