use crate::config::*;
use std::collections::HashMap;
use std::path::PathBuf;

fn minimal_config() -> ProcessConfig {
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
fn parse_minimal_config() {
    let toml = r#"
[processes.svc]
command = "echo hello"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert!(config.daemon.web.addr.is_some());
    assert_eq!(config.daemon.web.port, Some(9090));
    assert!(config.processes.contains_key("svc"));
    assert_eq!(config.processes["svc"].command, "echo hello");
}

#[test]
fn parse_full_process_config() {
    let toml = r#"
[processes.web]
command = "node"
args = ["app.js", "--port", "3000"]
working_dir = "/app"
auto_start = false
auto_restart = false
max_restarts = 5
restart_delay_secs = 10
stop_timeout_secs = 15
unhealthy_restart = true
depends_on = ["db"]
stdout_log = "logs/web.log"
stderr_log = "logs/web.err"

[processes.web.env]
NODE_ENV = "production"
PORT = "3000"

[processes.web.health_check]
type = "tcp"
port = 3000
interval_secs = 10
timeout_secs = 3
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let svc = &config.processes["web"];
    assert_eq!(svc.command, "node");
    assert_eq!(svc.args, vec!["app.js", "--port", "3000"]);
    assert!(!svc.auto_start);
    assert!(!svc.auto_restart);
    assert_eq!(svc.max_restarts, 5);
    assert_eq!(svc.restart_delay_secs, 10);
    assert_eq!(svc.stop_timeout_secs, 15);
    assert!(svc.unhealthy_restart);
    assert_eq!(svc.depends_on, vec!["db"]);
    assert_eq!(svc.env.get("NODE_ENV").unwrap(), "production");
    let hc = svc.health_check.as_ref().unwrap();
    assert_eq!(hc.interval_secs, 10);
    assert_eq!(hc.timeout_secs, 3);
}

#[test]
fn config_defaults() {
    let toml = r#"
[processes.svc]
command = "run"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let svc = &config.processes["svc"];
    assert!(svc.auto_start);
    assert!(svc.auto_restart);
    assert_eq!(svc.max_restarts, 3);
    assert_eq!(svc.restart_delay_secs, 5);
    assert_eq!(svc.stop_timeout_secs, 10);
    assert!(svc.args.is_empty());
    assert!(svc.env.is_empty());
    assert!(svc.health_check.is_none());
    assert!(!svc.unhealthy_restart);
}

#[test]
fn validate_empty_command() {
    let mut config = minimal_config();
    config.command = String::new();
    assert!(config.validate().is_err());
}

#[test]
fn validate_health_check_tcp_port_zero() {
    let mut config = minimal_config();
    config.health_check = Some(HealthCheckConfig {
        check_type: HealthCheckType::Tcp { host: None, port: 0 },
        interval_secs: 10,
        timeout_secs: 3,
    });
    assert!(config.validate().is_err());
}

#[test]
fn validate_health_check_http_empty_url() {
    let mut config = minimal_config();
    config.health_check = Some(HealthCheckConfig {
        check_type: HealthCheckType::Http { url: "  ".to_string() },
        interval_secs: 10,
        timeout_secs: 3,
    });
    assert!(config.validate().is_err());
}

#[test]
fn validate_health_check_zero_interval() {
    let mut config = minimal_config();
    config.health_check = Some(HealthCheckConfig {
        check_type: HealthCheckType::Tcp { host: None, port: 8080 },
        interval_secs: 0,
        timeout_secs: 3,
    });
    assert!(config.validate().is_err());
}

#[test]
fn validate_health_check_zero_timeout() {
    let mut config = minimal_config();
    config.health_check = Some(HealthCheckConfig {
        check_type: HealthCheckType::Tcp { host: None, port: 8080 },
        interval_secs: 10,
        timeout_secs: 0,
    });
    assert!(config.validate().is_err());
}

#[test]
fn validate_valid_config() {
    let mut config = minimal_config();
    config.health_check = Some(HealthCheckConfig {
        check_type: HealthCheckType::Tcp { host: Some("localhost".to_string()), port: 8080 },
        interval_secs: 10,
        timeout_secs: 3,
    });
    assert!(config.validate().is_ok());
}

#[test]
fn save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.toml");

    let mut processes = HashMap::new();
    let mut config = minimal_config();
    config.command = "node app.js".to_string();
    processes.insert("web".to_string(), config);

    let original = AppConfig {
        daemon: DaemonConfig {
            api_key: Some("secret".to_string()),
            ..DaemonConfig::default()
        },
        processes,
    };

    original.save(&path).unwrap();
    assert!(path.exists());

    let loaded = AppConfig::load(&path).unwrap();
    assert_eq!(loaded.daemon.api_key, Some("secret".to_string()));
    assert!(loaded.processes.contains_key("web"));
    assert_eq!(loaded.processes["web"].command, "node app.js");
}

#[test]
fn load_nonexistent_file_returns_default() {
    let config = AppConfig::load(std::path::Path::new("/nonexistent/gugu.toml")).unwrap();
    assert!(config.processes.is_empty());
}

#[test]
fn server_addr_default() {
    let config = AppConfig::default();
    assert_eq!(config.server_addr(), "0.0.0.0:9090");
}

#[test]
fn server_addr_custom() {
    let toml = r#"
[daemon.web]
addr = "192.168.1.1"
port = 8080
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.server_addr(), "192.168.1.1:8080");
}

#[test]
fn full_command_with_args() {
    let mut config = minimal_config();
    config.args = vec!["app.js".to_string(), "--port".to_string(), "3000".to_string()];
    assert_eq!(config.full_command(), "echo app.js --port 3000");
}

#[test]
fn full_command_without_args() {
    let config = minimal_config();
    assert_eq!(config.full_command(), "echo");
}

#[test]
fn runtime_fields_eq() {
    let a = minimal_config();
    let mut b = a.clone();
    assert!(a.runtime_fields_eq(&b));

    b.auto_start = false; // 非运行时字段
    assert!(a.runtime_fields_eq(&b));

    b.command = "python".to_string(); // 运行时字段
    assert!(!a.runtime_fields_eq(&b));
}

#[test]
fn normalize_paths_forward_slash() {
    let mut config = minimal_config();
    config.working_dir = Some(PathBuf::from(r"path\to\dir"));
    config.stdout_log = Some(PathBuf::from(r"logs\out.log"));
    config.stderr_log = Some(PathBuf::from(r"logs\err.log"));
    config.normalize_paths();
    assert_eq!(config.working_dir.unwrap().to_string_lossy(), "path/to/dir");
    assert_eq!(config.stdout_log.unwrap().to_string_lossy(), "logs/out.log");
}

#[test]
fn resolve_relative_path_absolute_unchanged() {
    let base = std::path::Path::new("/base");
    let path = std::path::Path::new("/absolute/path");
    let result = resolve_relative_path(path, base);
    assert_eq!(result, PathBuf::from("/absolute/path"));
}

#[test]
fn resolve_relative_path_relative_joined() {
    let base = std::path::Path::new("/base");
    let path = std::path::Path::new("relative/path");
    let result = resolve_relative_path(path, base);
    assert_eq!(result, PathBuf::from("/base/relative/path"));
}

#[test]
fn strip_unc_prefix_regular() {
    let path = PathBuf::from(r"C:\Users\test");
    let result = strip_unc_prefix(&path);
    assert_eq!(result, PathBuf::from(r"C:\Users\test"));
}

#[test]
fn strip_unc_prefix_extended() {
    let path = PathBuf::from(r"\\?\C:\Users\test");
    let result = strip_unc_prefix(&path);
    assert_eq!(result, PathBuf::from(r"C:\Users\test"));
}

#[test]
fn strip_unc_prefix_unc() {
    let path = PathBuf::from(r"\\?\UNC\server\share");
    let result = strip_unc_prefix(&path);
    assert_eq!(result, PathBuf::from(r"\\server\share"));
}

#[test]
fn convert_path_to_forward_slashes() {
    let path = PathBuf::from(r"a\b\c.txt");
    assert_eq!(path_to_forward_slashes(&path), "a/b/c.txt");
}
