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
        stop_command: None,
        stop_timeout_secs: 10,
        health_check: None,
        unhealthy_restart: false,
        depends_on: Vec::new(),
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

// ── LogStream 序列化 ──────────────────────────────────────────

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
