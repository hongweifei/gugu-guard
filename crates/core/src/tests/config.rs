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
        group: None,
        max_log_size_mb: None,
        stdout_log: None,
        stderr_log: None,
    }
}

// ── TOML 解析 ──────────────────────────────────────────────

mod parse_config {
    use super::*;

    #[test]
    fn parses_minimal_process_config() {
        let toml = r#"
[processes.svc]
command = "echo hello"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert!(
            config.processes.contains_key("svc"),
            "应包含 'svc' 进程"
        );
    }

    #[test]
    fn parses_command_field() {
        let toml = r#"
[processes.svc]
command = "echo hello"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.processes["svc"].command, "echo hello",
            "命令字段应正确解析"
        );
    }

    #[test]
    fn provides_default_web_port() {
        let toml = r#"
[processes.svc]
command = "echo hello"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.daemon.web.port,
            Some(9090),
            "默认端口应为 9090"
        );
    }

    #[test]
    fn parses_full_process_config() {
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

        assert_eq!(svc.command, "node", "command 应为 'node'");
        assert_eq!(svc.args, vec!["app.js", "--port", "3000"], "args 应正确解析");
        assert!(!svc.auto_start, "auto_start 应为 false");
        assert!(!svc.auto_restart, "auto_restart 应为 false");
        assert_eq!(svc.max_restarts, 5, "max_restarts 应为 5");
        assert_eq!(svc.restart_delay_secs, 10, "restart_delay_secs 应为 10");
        assert_eq!(svc.stop_timeout_secs, 15, "stop_timeout_secs 应为 15");
        assert!(svc.unhealthy_restart, "unhealthy_restart 应为 true");
        assert_eq!(svc.depends_on, vec!["db"], "depends_on 应为 ['db']");
        assert_eq!(
            svc.env.get("NODE_ENV").unwrap(),
            "production",
            "环境变量 NODE_ENV 应为 'production'"
        );

        let hc = svc.health_check.as_ref().expect("应配置健康检查");
        assert_eq!(hc.interval_secs, 10, "interval_secs 应为 10");
        assert_eq!(hc.timeout_secs, 3, "timeout_secs 应为 3");
    }

    #[test]
    fn provides_correct_defaults() {
        let toml = r#"
[processes.svc]
command = "run"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let svc = &config.processes["svc"];

        assert!(svc.auto_start, "默认 auto_start 应为 true");
        assert!(svc.auto_restart, "默认 auto_restart 应为 true");
        assert_eq!(svc.max_restarts, 3, "默认 max_restarts 应为 3");
        assert_eq!(svc.restart_delay_secs, 5, "默认 restart_delay_secs 应为 5");
        assert_eq!(svc.stop_timeout_secs, 10, "默认 stop_timeout_secs 应为 10");
        assert!(svc.args.is_empty(), "默认 args 应为空");
        assert!(svc.env.is_empty(), "默认 env 应为空");
        assert!(svc.health_check.is_none(), "默认 health_check 应为 None");
        assert!(!svc.unhealthy_restart, "默认 unhealthy_restart 应为 false");
    }
}

// ── 配置验证 ──────────────────────────────────────────────

mod validate {
    use super::*;

    #[test]
    fn rejects_empty_command() {
        let mut config = minimal_config();
        config.command = String::new();

        let err = config.validate().unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "空命令应返回有意义的错误信息"
        );
    }

    #[test]
    fn rejects_tcp_health_check_with_port_zero() {
        let mut config = minimal_config();
        config.health_check = Some(HealthCheckConfig {
            check_type: HealthCheckType::Tcp { host: None, port: 0 },
            interval_secs: 10,
            timeout_secs: 3,
        });

        assert!(
            config.validate().is_err(),
            "TCP 健康检查端口为 0 应验证失败"
        );
    }

    #[test]
    fn rejects_http_health_check_with_empty_url() {
        let mut config = minimal_config();
        config.health_check = Some(HealthCheckConfig {
            check_type: HealthCheckType::Http { url: "  ".to_string() },
            interval_secs: 10,
            timeout_secs: 3,
        });

        assert!(
            config.validate().is_err(),
            "HTTP 健康检查 URL 为空白应验证失败"
        );
    }

    #[test]
    fn rejects_health_check_with_zero_interval() {
        let mut config = minimal_config();
        config.health_check = Some(HealthCheckConfig {
            check_type: HealthCheckType::Tcp { host: None, port: 8080 },
            interval_secs: 0,
            timeout_secs: 3,
        });

        assert!(
            config.validate().is_err(),
            "健康检查间隔为 0 应验证失败"
        );
    }

    #[test]
    fn rejects_health_check_with_zero_timeout() {
        let mut config = minimal_config();
        config.health_check = Some(HealthCheckConfig {
            check_type: HealthCheckType::Tcp { host: None, port: 8080 },
            interval_secs: 10,
            timeout_secs: 0,
        });

        assert!(
            config.validate().is_err(),
            "健康检查超时为 0 应验证失败"
        );
    }

    #[test]
    fn accepts_valid_config_with_health_check() {
        let mut config = minimal_config();
        config.health_check = Some(HealthCheckConfig {
            check_type: HealthCheckType::Tcp {
                host: Some("localhost".to_string()),
                port: 8080,
            },
            interval_secs: 10,
            timeout_secs: 3,
        });

        assert!(
            config.validate().is_ok(),
            "有效配置应通过验证"
        );
    }
}

// ── 保存与加载 ──────────────────────────────────────────────

mod save_and_load {
    use super::*;

    #[test]
    fn roundtrips_config_to_file() {
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
        assert!(path.exists(), "保存后文件应存在");

        let loaded = AppConfig::load(&path).unwrap();
        assert_eq!(
            loaded.daemon.api_key,
            Some("secret".to_string()),
            "api_key 应一致"
        );
        assert!(
            loaded.processes.contains_key("web"),
            "应包含 'web' 进程"
        );
        assert_eq!(
            loaded.processes["web"].command,
            "node app.js",
            "command 应一致"
        );
    }

    #[test]
    fn returns_default_when_file_not_found() {
        let config = AppConfig::load(std::path::Path::new("/nonexistent/gugu.toml")).unwrap();
        assert!(
            config.processes.is_empty(),
            "加载不存在的文件应返回空配置"
        );
    }
}

// ── server_addr ──────────────────────────────────────────────

mod server_addr {
    use super::*;

    #[test]
    fn defaults_to_wildcard_9090() {
        let config = AppConfig::default();
        assert_eq!(
            config.server_addr(),
            "0.0.0.0:9090",
            "默认地址应为 0.0.0.0:9090"
        );
    }

    #[test]
    fn uses_custom_addr_and_port() {
        let toml = r#"
[daemon.web]
addr = "192.168.1.1"
port = 8080
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.server_addr(),
            "192.168.1.1:8080",
            "应使用自定义地址和端口"
        );
    }
}

// ── full_command ──────────────────────────────────────────────

mod full_command {
    use super::*;

    #[test]
    fn joins_command_with_args() {
        let mut config = minimal_config();
        config.args = vec!["app.js".to_string(), "--port".to_string(), "3000".to_string()];

        assert_eq!(
            config.full_command(),
            "echo app.js --port 3000",
            "应拼接命令和参数"
        );
    }

    #[test]
    fn returns_command_alone_when_no_args() {
        let config = minimal_config();
        assert_eq!(
            config.full_command(),
            "echo",
            "无参数时仅返回命令"
        );
    }
}

// ── runtime_fields_eq ────────────────────────────────────────

mod runtime_fields_eq {
    use super::*;

    #[test]
    fn ignores_non_runtime_field_changes() {
        let a = minimal_config();
        let mut b = a.clone();
        b.auto_start = false; // 非运行时字段

        assert!(
            a.runtime_fields_eq(&b),
            "非运行时字段变更不应影响相等性"
        );
    }

    #[test]
    fn detects_runtime_field_changes() {
        let a = minimal_config();
        let mut b = a.clone();
        b.command = "python".to_string(); // 运行时字段

        assert!(
            !a.runtime_fields_eq(&b),
            "运行时字段变更应被检测"
        );
    }

    #[test]
    fn identical_configs_are_equal() {
        let a = minimal_config();
        let b = a.clone();
        assert!(
            a.runtime_fields_eq(&b),
            "相同配置应相等"
        );
    }
}

// ── 路径处理 ──────────────────────────────────────────────

mod normalize_paths {
    use super::*;

    #[test]
    fn converts_backslashes_to_forward_slashes() {
        let mut config = minimal_config();
        config.working_dir = Some(PathBuf::from(r"path\to\dir"));
        config.stdout_log = Some(PathBuf::from(r"logs\out.log"));
        config.stderr_log = Some(PathBuf::from(r"logs\err.log"));
        config.normalize_paths();

        assert_eq!(
            config.working_dir.unwrap().to_string_lossy(),
            "path/to/dir",
            "working_dir 反斜杠应转为正斜杠"
        );
        assert_eq!(
            config.stdout_log.unwrap().to_string_lossy(),
            "logs/out.log",
            "stdout_log 反斜杠应转为正斜杠"
        );
    }
}

mod resolve_relative_path {
    use super::*;

    #[test]
    fn keeps_absolute_path_unchanged() {
        let base = std::path::Path::new("/base");
        let path = std::path::Path::new("/absolute/path");

        let result = resolve_relative_path(path, base);
        assert_eq!(
            result,
            PathBuf::from("/absolute/path"),
            "绝对路径应保持不变"
        );
    }

    #[test]
    fn joins_relative_path_with_base() {
        let base = std::path::Path::new("/base");
        let path = std::path::Path::new("relative/path");

        let result = resolve_relative_path(path, base);
        assert_eq!(
            result,
            PathBuf::from("/base/relative/path"),
            "相对路径应与 base 拼接"
        );
    }
}

mod strip_unc_prefix {
    use super::*;

    #[test]
    fn keeps_regular_path_unchanged() {
        let path = PathBuf::from(r"C:\Users\test");
        let result = strip_unc_prefix(&path);
        assert_eq!(
            result,
            PathBuf::from(r"C:\Users\test"),
            "常规路径应保持不变"
        );
    }

    #[test]
    fn strips_extended_prefix() {
        let path = PathBuf::from(r"\\?\C:\Users\test");
        let result = strip_unc_prefix(&path);
        assert_eq!(
            result,
            PathBuf::from(r"C:\Users\test"),
            r"应去除 \\?\ 前缀"
        );
    }

    #[test]
    fn converts_unc_prefix_to_standard() {
        let path = PathBuf::from(r"\\?\UNC\server\share");
        let result = strip_unc_prefix(&path);
        assert_eq!(
            result,
            PathBuf::from(r"\\server\share"),
            r"应将 \\?\UNC 转为 \\"
        );
    }
}

mod path_to_forward_slashes {
    use super::*;

    #[test]
    fn converts_backslashes() {
        let path = PathBuf::from(r"a\b\c.txt");
        assert_eq!(
            path_to_forward_slashes(&path),
            "a/b/c.txt",
            "反斜杠应转为正斜杠"
        );
    }
}
