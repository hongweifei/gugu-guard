//! PTY 的创建、shell 探测与输出解码。
//!
//! 每个会话一个 PTY：master 端留在服务里读写，slave 端交给 shell 子进程。
//! 读取放在独立线程里，通过 channel 交给会话循环，避免阻塞 tokio 运行时。

use std::path::Path;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

/// 读取 PTY 的缓冲区大小。
const READ_BUF: usize = 8192;
/// 会话与 PTY 读取线程之间的积压上限。
const OUTPUT_QUEUE: usize = 64;
/// 新建 PTY 的默认行列数；浏览器连上后会按面板尺寸调整。
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// 已启动的 PTY 及其读写端。
pub(super) struct Pty {
    pub(super) master: Box<dyn MasterPty + Send>,
    pub(super) writer: Box<dyn std::io::Write + Send>,
    pub(super) child: Box<dyn Child + Send + Sync>,
    pub(super) output_rx: mpsc::Receiver<Vec<u8>>,
    /// shell 的程序名（不含路径），用于提示与审计。
    pub(super) shell: String,
}

impl Drop for Pty {
    fn drop(&mut self) {
        // 无论正常结束、客户端断开还是任务被取消，都不留下孤立的 shell。
        let _ = self.child.kill();
    }
}

pub(super) fn spawn_pty(shell: &str, args: &[&str], cwd: &Path) -> anyhow::Result<Pty> {
    let pair = native_pty_system().openpty(PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(shell);
    for arg in args {
        command.arg(arg);
    }
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");

    let mut child = pair.slave.spawn_command(command)?;
    // slave 端交给子进程后即可关闭，否则子进程退出后 master 可能读不到 EOF。
    drop(pair.slave);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_QUEUE);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; READ_BUF];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!("读取终端输出结束: {error}");
                    break;
                }
            }
        }
        // 关闭发送端，让会话循环收到 None 并收尾。
        drop(output_tx);
    });

    // 进程退出码延迟到关闭审计时才用，先探测一次避免留下僵尸。
    let _ = child.try_wait();

    let shell_name = Path::new(shell).file_name().map_or_else(
        || shell.to_string(),
        |name| name.to_string_lossy().to_string(),
    );

    Ok(Pty {
        master: pair.master,
        writer,
        child,
        output_rx,
        shell: shell_name,
    })
}

/// 默认 shell 及其启动参数（Windows 用 PowerShell，其它平台用 `$SHELL`）。
pub(super) fn shell_command() -> (String, Vec<&'static str>) {
    #[cfg(windows)]
    {
        let pwsh = find_in_path("pwsh.exe");
        let shell = pwsh.unwrap_or_else(|| "powershell.exe".to_string());
        (shell, vec!["-NoLogo"])
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "/bin/bash".to_string());
        (shell, Vec::new())
    }
}

/// 默认 shell 的程序名，用于 `/api/v1/terminal` 的说明字段。
pub(super) fn shell_program() -> String {
    let (shell, _) = shell_command();
    file_name_of(&shell)
}

fn file_name_of(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}

#[cfg(windows)]
fn find_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
}

/// 取出字节流里已经完整的 UTF-8 前缀，未完成的多字节字符留在末尾等下一块。
/// 真遇到非法字节时用替换字符顶替，避免一个坏字节吞掉后面的输出。
pub(super) fn take_utf8_prefix(bytes: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(all) => {
                text.push_str(all);
                bytes.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    if let Ok(prefix) = std::str::from_utf8(&bytes[..valid]) {
                        text.push_str(prefix);
                    }
                    bytes.drain(..valid);
                }
                match error.error_len() {
                    Some(len) => {
                        text.push('\u{FFFD}');
                        bytes.drain(..len);
                    }
                    None => break,
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn utf8_output_split_across_chunks_is_reassembled() {
        let full = "咕咕鸽终端：你好，世界".as_bytes();
        for split in 1..full.len() {
            let mut pending = Vec::new();
            let mut text = String::new();
            pending.extend_from_slice(&full[..split]);
            text.push_str(&take_utf8_prefix(&mut pending));
            pending.extend_from_slice(&full[split..]);
            text.push_str(&take_utf8_prefix(&mut pending));
            assert_eq!(text, "咕咕鸽终端：你好，世界", "在字节 {split} 处切块");
            assert!(pending.is_empty(), "完整字符不应滞留");
        }
    }

    #[test]
    fn utf8_invalid_byte_does_not_swallow_the_rest() {
        let mut pending = vec![b'a', 0xff, b'b'];
        assert_eq!(take_utf8_prefix(&mut pending), "a\u{FFFD}b");
        assert!(pending.is_empty());
    }

    #[test]
    fn shell_program_is_a_bare_file_name() {
        let program = shell_program();
        assert!(!program.contains(['/', '\\']), "{program}");
        assert!(!program.is_empty());
    }

    #[test]
    fn interactive_pty_executes_commands() {
        let (shell, args) = shell_command();
        let cwd = std::env::current_dir().unwrap();
        let mut pty = match spawn_pty(&shell, &args, &cwd) {
            Ok(pty) => pty,
            // 受限沙箱（如桌面助手自己的运行时）禁止 conhost 启动，CreatePseudoConsole
            // 返回 ERROR_INVALID_HANDLE。普通用户会话不受影响，这里跳过而不是伪装通过。
            Err(error) if cfg!(windows) && error.to_string().contains("psuedo console") => {
                eprintln!("跳过 PTY 测试：当前进程无法创建 ConPTY（{error}）");
                return;
            }
            Err(error) => panic!("创建 PTY 失败: {error}"),
        };
        pty.writer.write_all(b"echo gugu-terminal-ok\r\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut output = String::new();
        while Instant::now() < deadline && !output.contains("gugu-terminal-ok\r\n") {
            match pty.output_rx.try_recv() {
                Ok(chunk) => output.push_str(&String::from_utf8_lossy(&chunk)),
                Err(mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        let _ = pty.child.kill();
        assert!(
            output.contains("gugu-terminal-ok"),
            "PTY 未回显命令输出，实际拿到: {output:?}"
        );
    }
}
