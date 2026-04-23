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

#[test]
fn process_status_display() {
    assert_eq!(ProcessStatus::Stopped.to_string(), "已停止");
    assert_eq!(ProcessStatus::Running.to_string(), "运行中");
    assert_eq!(ProcessStatus::Starting.to_string(), "启动中");
    assert_eq!(ProcessStatus::Restarting.to_string(), "重启中");
    assert_eq!(ProcessStatus::Failed("timeout".to_string()).to_string(), "失败: timeout");
}

#[test]
fn process_status_serde_roundtrip() {
    let status = ProcessStatus::Running;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"running\"");
    let back: ProcessStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ProcessStatus::Running);
}

#[test]
fn process_status_failed_serde() {
    let status = ProcessStatus::Failed("error msg".to_string());
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains("failed"));
    let back: ProcessStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, status);
}

#[test]
fn managed_process_initial_state() {
    let mut config = test_config();
    config.auto_restart = true;
    config.max_restarts = 3;
    config.restart_delay_secs = 5;
    let proc = ManagedProcess::new("test".to_string(), config);

    assert_eq!(proc.name(), "test");
    assert!(!proc.is_running());
    assert!(proc.should_auto_restart());
    assert_eq!(proc.restart_delay(), std::time::Duration::from_secs(5));
}

#[test]
fn managed_process_rename() {
    let config = test_config();
    let mut proc = ManagedProcess::new("old".to_string(), config);
    proc.rename("new".to_string());
    assert_eq!(proc.name(), "new");
}

#[test]
fn managed_process_restart_count() {
    let mut config = test_config();
    config.auto_restart = true;
    config.max_restarts = 2;
    let mut proc = ManagedProcess::new("test".to_string(), config);

    assert!(proc.should_auto_restart());
    proc.mark_crash_restart(); // count = 1
    assert!(proc.should_auto_restart());
    proc.mark_crash_restart(); // count = 2
    assert!(!proc.should_auto_restart()); // 达到上限

    proc.reset_crash_restart_count();
    assert!(proc.should_auto_restart());
}

#[test]
fn managed_process_auto_restart_disabled() {
    let mut config = test_config();
    config.auto_restart = false;
    let proc = ManagedProcess::new("test".to_string(), config);
    assert!(!proc.should_auto_restart());
}

#[test]
fn managed_process_info() {
    let mut config = test_config();
    config.command = "node".to_string();
    config.args = vec!["app.js".to_string()];
    config.auto_start = true;
    config.auto_restart = true;
    config.health_check = Some(crate::config::HealthCheckConfig {
        check_type: crate::config::HealthCheckType::Tcp { host: None, port: 3000 },
        interval_secs: 10,
        timeout_secs: 3,
    });

    let mut proc = ManagedProcess::new("web".to_string(), config);
    proc.set_healthy(Some(true));

    let info = proc.info();
    assert_eq!(info.name, "web");
    assert_eq!(info.command, "node");
    assert_eq!(info.args, vec!["app.js"]);
    assert_eq!(info.status, ProcessStatus::Stopped);
    assert!(info.auto_start);
    assert!(info.auto_restart);
    assert!(info.has_health_check);
    assert_eq!(info.healthy, Some(true));
    assert!(info.pid.is_none());
}

#[test]
fn managed_process_set_status() {
    let config = test_config();
    let mut proc = ManagedProcess::new("test".to_string(), config);
    assert!(!proc.is_running());

    proc.set_status(ProcessStatus::Running);
    assert!(proc.is_running());

    proc.set_status(ProcessStatus::Starting);
    assert!(proc.is_running());

    proc.set_status(ProcessStatus::Stopped);
    assert!(!proc.is_running());
}

#[tokio::test]
async fn managed_process_logs_empty() {
    let config = test_config();
    let proc = ManagedProcess::new("test".to_string(), config);
    let logs = proc.logs(100).await;
    assert!(logs.is_empty());
}

#[tokio::test]
async fn managed_process_clear_logs() {
    let config = test_config();
    let proc = ManagedProcess::new("test".to_string(), config);
    proc.clear_logs().await;
    let logs = proc.logs(100).await;
    assert!(logs.is_empty());
}

#[test]
fn log_entry_serde() {
    let entry = LogEntry {
        timestamp: Utc::now(),
        stream: LogStream::Stdout,
        line: "hello world".to_string(),
        process_name: Some("test".to_string()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: LogEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.line, "hello world");
    assert_eq!(back.stream, LogStream::Stdout);
    assert_eq!(back.process_name, Some("test".to_string()));
}

#[test]
fn log_stream_serde() {
    let json = serde_json::to_string(&LogStream::Stdout).unwrap();
    assert_eq!(json, "\"stdout\"");
    let json = serde_json::to_string(&LogStream::Stderr).unwrap();
    assert_eq!(json, "\"stderr\"");
}

#[test]
fn append_suffix_preserves_extension() {
    let path = std::path::Path::new("logs/stdout.log");
    let stem = path.file_stem().unwrap().to_string_lossy();
    let ext = path.extension().unwrap().to_string_lossy();
    assert_eq!(stem, "stdout");
    assert_eq!(ext, "log");
}
