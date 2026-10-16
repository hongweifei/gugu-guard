use crate::config::ProcessConfig;
use crate::process::{LogEntry, LogStream, ManagedProcess, ProcessStatus};
use chrono::Utc;
use std::collections::HashMap;

fn test_config() -> ProcessConfig {
    ProcessConfig {
        command: "echo".to_string(),
        args: Vec::new(),
        working_dir: None,
        env: HashMap::new(),
        auto_start: true,
        auto_restart: true,
        max_restarts: 3,
        restart_delay_secs: 5,
        restart_reset_secs: 60,
        shell: false,
        stop_command: None,
        stop_timeout_secs: 10,
        health_check: None,
        unhealthy_restart: false,
        depends_on: Vec::new(),
        group: None,
        max_log_size_mb: None,
        stdout_log: None,
        stderr_log: None,
    }
}

// ── ProcessStatus Display ───────────────────────────────────

mod process_status_display {
    use super::*;

    #[test]
    fn displays_stopped_in_chinese() {
        assert_eq!(ProcessStatus::Stopped.to_string(), "已停止");
    }

    #[test]
    fn displays_running_in_chinese() {
        assert_eq!(ProcessStatus::Running.to_string(), "运行中");
    }

    #[test]
    fn displays_starting_in_chinese() {
        assert_eq!(ProcessStatus::Starting.to_string(), "启动中");
    }

    #[test]
    fn displays_restarting_in_chinese() {
        assert_eq!(ProcessStatus::Restarting.to_string(), "重启中");
    }

    #[test]
    fn displays_failed_with_reason() {
        let status = ProcessStatus::Failed("timeout".to_string());
        assert_eq!(status.to_string(), "失败: timeout", "应包含失败原因");
    }
}

// ── ProcessStatus 序列化 ────────────────────────────────────

mod process_status_serde {
    use super::*;

    #[test]
    fn roundtrips_running_status() {
        let json = serde_json::to_string(&ProcessStatus::Running).unwrap();
        assert_eq!(json, "\"running\"", "Running 应序列化为小写");

        let back: ProcessStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProcessStatus::Running, "反序列化应还原");
    }

    #[test]
    fn roundtrips_failed_status() {
        let status = ProcessStatus::Failed("error msg".to_string());
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("failed"), "Failed 应包含 'failed'");

        let back: ProcessStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status, "反序列化应还原");
    }
}

// ── ManagedProcess 初始状态 ─────────────────────────────────

mod managed_process_initial_state {
    use super::*;

    #[test]
    fn has_correct_name() {
        let proc = ManagedProcess::new("test".to_string(), test_config());
        assert_eq!(proc.name(), "test", "名称应为 'test'");
    }

    #[test]
    fn is_not_running() {
        let proc = ManagedProcess::new("test".to_string(), test_config());
        assert!(!proc.is_running(), "初始状态应为非运行");
    }

    #[test]
    fn should_auto_restart_when_enabled() {
        let mut config = test_config();
        config.auto_restart = true;
        config.max_restarts = 3;
        let proc = ManagedProcess::new("test".to_string(), config);

        assert!(proc.should_auto_restart(), "启用时应允许自动重启");
    }

    #[test]
    fn returns_configured_restart_delay() {
        let mut config = test_config();
        config.restart_delay_secs = 5;
        let proc = ManagedProcess::new("test".to_string(), config);

        assert_eq!(
            proc.restart_delay(),
            std::time::Duration::from_secs(5),
            "重启延迟应为 5 秒"
        );
    }
}

// ── ManagedProcess rename ────────────────────────────────────

mod managed_process_rename {
    use super::*;

    #[test]
    fn updates_name() {
        let config = test_config();
        let mut proc = ManagedProcess::new("old".to_string(), config);
        proc.rename("new".to_string());
        assert_eq!(proc.name(), "new", "名称应更新为 'new'");
    }
}

// ── ManagedProcess restart count ─────────────────────────────

mod managed_process_restart_count {
    use super::*;

    fn make_proc(max_restarts: u32) -> ManagedProcess {
        let mut config = test_config();
        config.auto_restart = true;
        config.max_restarts = max_restarts;
        ManagedProcess::new("test".to_string(), config)
    }

    #[test]
    fn allows_restart_below_limit() {
        let mut proc = make_proc(2);
        proc.mark_crash_restart(); // count = 1
        assert!(proc.should_auto_restart(), "未达上限时应允许重启");
    }

    #[test]
    fn blocks_restart_at_limit() {
        let mut proc = make_proc(2);
        proc.mark_crash_restart(); // count = 1
        proc.mark_crash_restart(); // count = 2
        assert!(!proc.should_auto_restart(), "达到上限时不应允许重启");
    }

    #[test]
    fn resets_count_on_request() {
        let mut proc = make_proc(2);
        proc.mark_crash_restart();
        proc.mark_crash_restart();
        assert!(!proc.should_auto_restart());

        proc.reset_crash_restart_count();
        assert!(
            proc.should_auto_restart(),
            "重置后应允许重启"
        );
    }

    #[test]
    fn disabled_auto_restart_never_restarts() {
        let mut config = test_config();
        config.auto_restart = false;
        let proc = ManagedProcess::new("test".to_string(), config);
        assert!(
            !proc.should_auto_restart(),
            "禁用自动重启时永不允许重启"
        );
    }
}

// ── ManagedProcess info ──────────────────────────────────────

mod managed_process_info {
    use super::*;

    #[test]
    fn reflects_config_and_state() {
        let mut config = test_config();
        config.command = "node".to_string();
        config.args = vec!["app.js".to_string()];
        config.health_check = Some(crate::config::HealthCheckConfig {
            check_type: crate::config::HealthCheckType::Tcp { host: None, port: 3000 },
            interval_secs: 10,
            timeout_secs: 3,
            fail_threshold: 3,
        });

        let mut proc = ManagedProcess::new("web".to_string(), config);
        proc.set_healthy(Some(true));

        let info = proc.info();
        assert_eq!(info.name, "web", "名称应为 'web'");
        assert_eq!(info.command, "node", "命令应为 'node'");
        assert_eq!(info.args, vec!["app.js"], "参数应为 ['app.js']");
        assert_eq!(info.status, ProcessStatus::Stopped, "状态应为 Stopped");
        assert!(info.auto_start, "auto_start 应为 true");
        assert!(info.auto_restart, "auto_restart 应为 true");
        assert!(info.has_health_check, "应有健康检查");
        assert_eq!(info.healthy, Some(true), "健康状态应为 Some(true)");
        assert!(info.pid.is_none(), "PID 应为 None");
    }
}

// ── ManagedProcess set_status ────────────────────────────────

mod managed_process_set_status {
    use super::*;

    #[test]
    fn running_means_is_running() {
        let config = test_config();
        let mut proc = ManagedProcess::new("test".to_string(), config);

        proc.set_status(ProcessStatus::Running);
        assert!(proc.is_running(), "Running 状态应为运行中");
    }

    #[test]
    fn starting_means_is_running() {
        let config = test_config();
        let mut proc = ManagedProcess::new("test".to_string(), config);

        proc.set_status(ProcessStatus::Starting);
        assert!(proc.is_running(), "Starting 状态应为运行中");
    }

    #[test]
    fn stopped_means_not_running() {
        let config = test_config();
        let mut proc = ManagedProcess::new("test".to_string(), config);
        proc.set_status(ProcessStatus::Running);

        proc.set_status(ProcessStatus::Stopped);
        assert!(!proc.is_running(), "Stopped 状态应为非运行");
    }

    #[test]
    fn initial_state_is_not_running() {
        let config = test_config();
        let proc = ManagedProcess::new("test".to_string(), config);
        assert!(!proc.is_running(), "初始状态应为非运行");
    }
}

// ── ManagedProcess logs ──────────────────────────────────────

mod managed_process_logs {
    use super::*;

    #[tokio::test]
    async fn new_process_has_empty_logs() {
        let proc = ManagedProcess::new("test".to_string(), test_config());
        let logs = proc.logs(100).await;
        assert!(logs.is_empty(), "新建进程日志应为空");
    }

    #[tokio::test]
    async fn clear_logs_on_empty_is_harmless() {
        let proc = ManagedProcess::new("test".to_string(), test_config());
        proc.clear_logs().await;
        let logs = proc.logs(100).await;
        assert!(logs.is_empty(), "清空空日志应无副作用");
    }
}

// ── LogEntry 序列化 ──────────────────────────────────────────

mod log_entry_serde {
    use super::*;

    #[test]
    fn roundtrips_all_fields() {
        let entry = LogEntry {
            timestamp: Utc::now(),
            stream: LogStream::Stdout,
            line: "hello world".to_string(),
            process_name: Some("test".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(back.line, "hello world", "行内容应一致");
        assert_eq!(back.stream, LogStream::Stdout, "流类型应一致");
        assert_eq!(back.process_name, Some("test".to_string()), "进程名应一致");
    }
}

// ── 健康检查连续失败判定 (缺陷5) ──────────────────────────────

mod health_fail_threshold {
    use super::*;

    fn hc_config() -> ProcessConfig {
        let mut c = test_config();
        c.unhealthy_restart = true;
        c.health_check = Some(crate::config::HealthCheckConfig {
            check_type: crate::config::HealthCheckType::Tcp { host: None, port: 80 },
            interval_secs: 10,
            timeout_secs: 3,
            fail_threshold: 3,
        });
        c
    }

    #[test]
    fn does_not_trip_below_threshold() {
        let mut p = ManagedProcess::new("w".into(), hc_config());
        assert_eq!(p.record_health(false), (1, false), "第 1 次失败不应判定");
        assert_eq!(p.record_health(false), (2, false), "第 2 次失败不应判定");
    }

    #[test]
    fn trips_at_threshold() {
        let mut p = ManagedProcess::new("w".into(), hc_config());
        p.record_health(false);
        p.record_health(false);
        assert_eq!(p.record_health(false), (3, true), "第 3 次 (达阈值) 应判定");
    }

    #[test]
    fn success_resets_consecutive_failures() {
        let mut p = ManagedProcess::new("w".into(), hc_config());
        p.record_health(false);
        p.record_health(false);
        assert_eq!(p.record_health(true), (0, false), "成功应清零连续失败计数");
        assert_eq!(p.record_health(false), (1, false), "清零后应重新从 1 数起");
    }
}

// ── 崩溃重启计数衰减 (缺陷4) ──────────────────────────────────

mod crash_count_decay {
    use super::*;

    fn running_config() -> ProcessConfig {
        let mut c = test_config();
        c.restart_reset_secs = 60;
        c
    }

    #[test]
    fn resets_after_stable_uptime() {
        let mut p = ManagedProcess::new("w".into(), running_config());
        p.mark_crash_restart();
        p.set_status(ProcessStatus::Running);
        assert!(!p.maybe_decay_crash_count(30), "未达稳定时长不应衰减");
        assert!(p.should_auto_restart(), "崩溃 1 次且未到衰减点，计数应为 1");
        assert!(p.maybe_decay_crash_count(61), "达到稳定运行阈值应衰减");
        assert!(!p.maybe_decay_crash_count(999), "计数已为 0，重复调用应返回 false");
    }

    #[test]
    fn does_not_reset_when_not_running() {
        let mut p = ManagedProcess::new("w".into(), running_config());
        p.mark_crash_restart();
        p.set_status(ProcessStatus::Stopped);
        assert!(!p.maybe_decay_crash_count(999), "非运行态不应衰减计数");
    }
}

// ── 命令分词 (缺陷7：直接 spawn 不经 shell) ───────────────────

mod command_split {
    use crate::process::split_command_words;

    #[test]
    fn splits_plain_command_into_exe_and_leading_args() {
        let (exe, lead) = split_command_words("node app.js");
        assert_eq!(exe, "node");
        assert_eq!(lead, vec!["app.js".to_string()]);
    }

    #[test]
    fn respects_double_quotes_for_path_with_spaces() {
        let (exe, lead) = split_command_words("\"C:\\Program Files\\a.exe\" --flag");
        assert_eq!(exe, "C:\\Program Files\\a.exe", "引号内空格不应被拆开");
        assert_eq!(lead, vec!["--flag".to_string()]);
    }

    #[test]
    fn handles_empty_and_single() {
        let (exe, lead) = split_command_words("");
        assert_eq!(exe, "");
        assert!(lead.is_empty());
        let (exe, lead) = split_command_words("java");
        assert_eq!(exe, "java");
        assert!(lead.is_empty());
    }
}

// ── LogStream 序列化 ──────────────────────────────────────────

mod read_stream_non_utf8 {
    use super::*;
    use crate::process::read_stream;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::{broadcast, Mutex};

    #[tokio::test]
    async fn continues_after_invalid_utf8_line() {
        // 三行：有效 / 非法 UTF-8 (0xFF 0xFE) / 有效
        // 旧实现遇非法行 read_line 返回 Err 后 break，"last" 永远进不了缓冲
        let bytes: Vec<u8> = vec![
            b'f', b'i', b'r', b's', b't', b'\n',
            0xFF, 0xFE, b'\n',
            b'l', b'a', b's', b't', b'\n',
        ];
        let reader = std::io::Cursor::new(bytes);
        let buffer: Arc<Mutex<VecDeque<LogEntry>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, _rx) = broadcast::channel(16);
        read_stream(reader, None, buffer.clone(), tx, LogStream::Stdout, "t", None).await;

        let buf = buffer.lock().await;
        let lines: Vec<String> = buf.iter().map(|e| e.line.clone()).collect();
        assert!(
            lines.iter().any(|l| l.contains("first")),
            "首行有效内容应被捕获，实际: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("last")),
            "遇到非法 UTF-8 后应继续读取后续行，实际: {lines:?}"
        );
    }
}

mod log_stream_serde {
    use super::*;

    #[test]
    fn stdout_serializes_to_lowercase() {
        let json = serde_json::to_string(&LogStream::Stdout).unwrap();
        assert_eq!(json, "\"stdout\"", "Stdout 应序列化为小写");
    }

    #[test]
    fn stderr_serializes_to_lowercase() {
        let json = serde_json::to_string(&LogStream::Stderr).unwrap();
        assert_eq!(json, "\"stderr\"", "Stderr 应序列化为小写");
    }
}

// ── 直接启动 (缺陷7) Windows 实跑对照 ────────────────────────

#[cfg(windows)]
mod direct_spawn_windows {
    use super::*;
    use std::time::{Duration, Instant};

    async fn run_and_capture(mut p: ManagedProcess, expect: &str) -> Vec<String> {
        p.start().await.expect("应能启动");
        let deadline = Instant::now() + Duration::from_secs(8);
        let lines = loop {
            let logs = p.logs(64).await;
            let ls: Vec<String> = logs.iter().map(|e| e.line.clone()).collect();
            if ls.iter().any(|l| l.contains(expect)) || Instant::now() >= deadline {
                break ls;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        };
        if p.is_running() {
            let _ = p.stop().await;
        }
        lines
    }

    #[tokio::test]
    async fn space_in_arg_survives_direct_spawn_via_bat() {
        let dir = tempfile::tempdir().unwrap();
        let bat = dir.path().join("echoarg.bat");
        std::fs::write(&bat, "@echo [%~1]\r\n@ping -n 5 127.0.0.1 > nul\r\n").unwrap();
        let mut c = test_config();
        c.command = bat.display().to_string();
        c.args = vec!["a b c".to_string()];
        let lines = run_and_capture(ManagedProcess::new("bat".into(), c), "[").await;
        assert!(
            lines.iter().any(|l| l.trim() == "[a b c]"),
            "直接启动时含空格的参数应完整传入 (%~1)，实际: {lines:?}"
        );
    }

    #[tokio::test]
    async fn shell_false_does_not_execute_metacharacter() {
        let mut c = test_config();
        c.command = "cmd".to_string();
        c.args = vec!["/D".into(), "/C".into(), "echo".into(), "A & echo B".into()];
        let lines = run_and_capture(ManagedProcess::new("m1".into(), c), "A").await;
        assert!(
            lines.iter().any(|l| l.contains("A & echo B")),
            "& 应作为字面量输出，实际: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.trim() == "B"),
            "被注入的第二条命令不应被执行，实际: {lines:?}"
        );
    }

    #[tokio::test]
    async fn shell_true_still_executes_metacharacter() {
        let mut c = test_config();
        c.shell = true;
        c.command = "cmd".into();
        c.args = vec!["/D".into(), "/C".into(), "echo A & echo B".into()];
        let lines = run_and_capture(ManagedProcess::new("m2".into(), c), "B").await;
        assert!(
            lines.iter().any(|l| l.trim() == "B"),
            "shell = true 应保持拼接执行语义，实际: {lines:?}"
        );
    }
}
