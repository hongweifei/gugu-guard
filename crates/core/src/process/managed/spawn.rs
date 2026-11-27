//! 启动：命令构造、硬限落地与日志采集接线。

use chrono::Utc;
use tokio::process::Command;

use crate::cgroup;
use crate::error::{GuguError, Result};
use crate::process::capture::{read_stream, StreamTarget};
use crate::process::command::split_command_words;
use crate::process::types::{LogStream, ProcessStatus};
use crate::sockets;

use super::ManagedProcess;

#[cfg(windows)]
use crate::process::command::{
    is_bat_cmd, resolve_program, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
};

impl ManagedProcess {
    pub async fn start(&mut self) -> Result<()> {
        if self.is_running() {
            return Err(GuguError::AlreadyRunning(self.name.clone()));
        }

        // 端口预检：端口被别的进程占着时，启动必然失败，先明确告诉用户是谁占的
        if self.config.port_check {
            for port in self.watched_ports() {
                if let Some(listener) = sockets::listen_on(port) {
                    let reason = listener.describe();
                    tracing::error!("[{}] 启动前端口检查失败: {}", self.name, reason);
                    self.status = ProcessStatus::Failed(reason.clone());
                    return Err(GuguError::StartFailed(self.name.clone(), reason));
                }
            }
        }

        self.status = ProcessStatus::Starting;
        self.abort_log_tasks();

        // 硬限先落地：载体建不出来或限值写不进去，就不该让进程跑起来。
        let spec = match self.config.cgroup_spec() {
            Ok(spec) => spec,
            Err(e) => {
                let reason = e.to_string();
                tracing::error!("[{}] 硬限配置无效: {}", self.name, reason);
                self.status = ProcessStatus::Failed(reason.clone());
                return Err(GuguError::StartFailed(self.name.clone(), reason));
            }
        };
        let mut limiter = match cgroup::prepare(&self.cgroup_base, &self.name, &spec) {
            Ok(limiter) => limiter,
            Err(reason) => {
                tracing::error!("[{}] 硬限创建失败: {}", self.name, reason);
                self.status = ProcessStatus::Failed(reason.clone());
                return Err(GuguError::StartFailed(self.name.clone(), reason));
            }
        };

        // shell=true 时保持旧行为：command+args 拼接后交给 shell (支持管道等)。
        // 默认 (shell=false) 直接启动可执行文件，参数以数组传递，
        // 空格路径不再被拆碎，参数中的 & | 等也不会被当作命令执行。
        let mut cmd;
        if self.config.shell {
            let full_cmd = if self.config.args.is_empty() {
                self.config.command.clone()
            } else {
                let args_str = self.config.args.join(" ");
                format!("{} {args_str}", self.config.command)
            };
            #[cfg(windows)]
            {
                cmd = Command::new("cmd");
                cmd.arg("/C").arg(&full_cmd);
                cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            }
            #[cfg(unix)]
            {
                cmd = Command::new("sh");
                cmd.arg("-c").arg(&full_cmd);
                cmd.process_group(0);
            }
        } else {
            let (exe, mut argv) = split_command_words(&self.config.command);
            if exe.is_empty() {
                self.status = ProcessStatus::Failed("command 为空，无法启动".into());
                return Err(GuguError::StartFailed(
                    self.name.clone(),
                    "command 为空".into(),
                ));
            }
            argv.extend(self.config.args.iter().cloned());
            #[cfg(windows)]
            {
                match resolve_program(&exe) {
                    Some(target) if is_bat_cmd(&target) => {
                        if argv.iter().any(|a| a.contains('"')) {
                            tracing::warn!(
                                "[{}] .bat/.cmd 包装执行时，参数中的双引号可能破坏 cmd 解析，建议改用 shell = true",
                                self.name
                            );
                        }
                        cmd = Command::new("cmd");
                        cmd.arg("/D").arg("/C").arg("call").arg(&target);
                    }
                    Some(target) => cmd = Command::new(target),
                    None => cmd = Command::new(&exe),
                }
                cmd.args(&argv);
                cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            }
            #[cfg(unix)]
            {
                cmd = Command::new(&exe);
                cmd.args(&argv);
                cmd.process_group(0);
            }
        }

        cmd.envs(&self.config.env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }

        // Linux：fork 之后、exec 之前把自己写进 cgroup.procs，进程树从第一条指令起就受限。
        #[cfg(target_os = "linux")]
        if limiter.is_some() {
            let attached = limiter
                .as_ref()
                .map_or(Ok(()), |limiter| limiter.attach(&mut cmd));
            if let Err(reason) = attached {
                if let Some(limiter) = limiter.take() {
                    limiter.kill_and_remove();
                }
                tracing::error!("[{}] 无法把进程加入 cgroup: {}", self.name, reason);
                self.status = ProcessStatus::Failed(reason.clone());
                return Err(GuguError::StartFailed(self.name.clone(), reason));
            }
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                // Windows：在这里把主进程分配进 Job；Linux 的子进程已在 exec 前落位。
                if let Some(active) = limiter.as_mut() {
                    if let Err(reason) = active.finish_spawn(pid) {
                        // 配了硬限就必须真的生效；只有未配限值时的 Job 失败可以降级运行。
                        if spec.is_active() {
                            if let Some(active) = limiter.take() {
                                active.kill_and_remove();
                            }
                            tracing::error!("[{}] 硬限未生效: {}", self.name, reason);
                            self.status = ProcessStatus::Failed(reason.clone());
                            return Err(GuguError::StartFailed(self.name.clone(), reason));
                        }
                        tracing::warn!("[{}] {}", self.name, reason);
                    }
                }
                self.limiter = limiter;

                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let max_log_size = self.config.max_log_size_mb;
                let format = self.config.log_format;
                let encoding = self.config.encoding;

                if let Some(stdout) = stdout {
                    let name = self.name.clone();
                    let log_file = self.config.stdout_log.clone();
                    let buffer = self.stdout_lines.clone();
                    let log_tx = self.log_tx.clone();
                    let watcher = self.log_watcher.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(
                            stdout,
                            LogStream::Stdout,
                            StreamTarget {
                                log_path: log_file,
                                buffer,
                                log_tx,
                                process: &name,
                                max_log_size_mb: max_log_size,
                                watcher,
                                format,
                                encoding,
                            },
                        )
                        .await;
                    });
                    self.log_tasks.push(handle);
                }

                if let Some(stderr) = stderr {
                    let name = self.name.clone();
                    let log_file = self.config.stderr_log.clone();
                    let buffer = self.stderr_lines.clone();
                    let log_tx = self.log_tx.clone();
                    let watcher = self.log_watcher.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(
                            stderr,
                            LogStream::Stderr,
                            StreamTarget {
                                log_path: log_file,
                                buffer,
                                log_tx,
                                process: &name,
                                max_log_size_mb: max_log_size,
                                watcher,
                                format,
                                encoding,
                            },
                        )
                        .await;
                    });
                    self.log_tasks.push(handle);
                }

                self.child = Some(child);
                let started_at = Utc::now();
                self.started_at = Some(started_at);
                self.history.mark_start(&self.name, started_at);
                self.status = ProcessStatus::Running;
                self.last_health_check = None;
                self.health_failures = 0;
                self.usage = None;
                self.connections = 0;

                tracing::info!("[{}] 进程已启动 (PID: {:?})", self.name, pid);
                Ok(())
            }
            Err(e) => {
                if let Some(limiter) = limiter {
                    limiter.kill_and_remove();
                }
                self.status = ProcessStatus::Failed(e.to_string());
                tracing::error!("[{}] 启动失败: {}", self.name, e);
                Err(GuguError::StartFailed(self.name.clone(), e.to_string()))
            }
        }
    }
}
