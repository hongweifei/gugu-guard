use crate::config::*;
use crate::manager::{topological_sort, SharedManager};
use std::collections::HashMap;

fn test_config() -> ProcessConfig {
    ProcessConfig {
        command: "echo".to_string(),
        args: Vec::new(),
        working_dir: None,
        env: HashMap::new(),
        auto_start: false,
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

fn make_proc(name: &str, depends_on: Vec<&str>) -> (String, ProcessConfig) {
    (
        name.to_string(),
        ProcessConfig {
            command: format!("run_{name}"),
            depends_on: depends_on.into_iter().map(String::from).collect(),
            ..test_config()
        },
    )
}

fn spawn_test_manager() -> SharedManager {
    let config = AppConfig::default();
    crate::manager::start(&config, None)
}

fn spawn_test_manager_with_path(dir: &std::path::Path) -> SharedManager {
    let config = AppConfig::default();
    crate::manager::start(&config, Some(dir.join("gugu.toml")))
}

// ── 拓扑排序 ──────────────────────────────────────────────

mod topological_sort {
    use super::*;

    #[test]
    fn returns_all_names_when_no_deps() {
        let mut map = HashMap::new();
        let (k1, v1) = make_proc("a", vec![]);
        let (k2, v2) = make_proc("b", vec![]);
        let (k3, v3) = make_proc("c", vec![]);
        map.insert(k1, v1);
        map.insert(k2, v2);
        map.insert(k3, v3);

        let result = topological_sort(&map).unwrap();
        assert_eq!(result.len(), 3, "应有 3 个进程，实际: {result:?}");
    }

    #[test]
    fn respects_linear_chain_order() {
        let mut map = HashMap::new();
        let (k1, v1) = make_proc("a", vec![]);
        let (k2, v2) = make_proc("b", vec!["a"]);
        let (k3, v3) = make_proc("c", vec!["b"]);
        map.insert(k1, v1);
        map.insert(k2, v2);
        map.insert(k3, v3);

        let result = topological_sort(&map).unwrap();
        assert!(
            result.iter().position(|n| n == "a").unwrap() < result.iter().position(|n| n == "b").unwrap(),
            "a 应排在 b 之前，实际顺序: {result:?}"
        );
        assert!(
            result.iter().position(|n| n == "b").unwrap() < result.iter().position(|n| n == "c").unwrap(),
            "b 应排在 c 之前，实际顺序: {result:?}"
        );
    }

    #[test]
    fn respects_diamond_dependency_order() {
        let mut map = HashMap::new();
        let (k1, v1) = make_proc("a", vec![]);
        let (k2, v2) = make_proc("b", vec!["a"]);
        let (k3, v3) = make_proc("c", vec!["a"]);
        let (k4, v4) = make_proc("d", vec!["b", "c"]);
        map.insert(k1, v1);
        map.insert(k2, v2);
        map.insert(k3, v3);
        map.insert(k4, v4);

        let result = topological_sort(&map).unwrap();
        let pos = |name: &str| result.iter().position(|n| n == name).unwrap();
        assert!(pos("a") < pos("b"), "a 应在 b 之前");
        assert!(pos("a") < pos("c"), "a 应在 c 之前");
        assert!(pos("b") < pos("d"), "b 应在 d 之前");
        assert!(pos("c") < pos("d"), "c 应在 d 之前");
    }

    #[test]
    fn detects_cycle() {
        let mut map = HashMap::new();
        let (k1, v1) = make_proc("a", vec!["b"]);
        let (k2, v2) = make_proc("b", vec!["c"]);
        let (k3, v3) = make_proc("c", vec!["a"]);
        map.insert(k1, v1);
        map.insert(k2, v2);
        map.insert(k3, v3);

        let err = topological_sort(&map).unwrap_err();
        assert!(
            err.to_string().contains("循环依赖"),
            "应包含循环依赖描述，实际: {err}"
        );
    }

    #[test]
    fn ignores_missing_dependency() {
        let mut map = HashMap::new();
        let (k, v) = make_proc("a", vec!["nonexistent"]);
        map.insert(k, v);

        let result = topological_sort(&map).unwrap();
        assert_eq!(result, vec!["a"]);
    }

    #[test]
    fn detects_self_cycle() {
        let mut map = HashMap::new();
        let (k, v) = make_proc("a", vec!["a"]);
        map.insert(k, v);

        assert!(topological_sort(&map).is_err(), "自依赖应被视为循环");
    }

    #[test]
    fn returns_empty_for_empty_input() {
        let map: HashMap<String, ProcessConfig> = HashMap::new();
        let result = topological_sort(&map).unwrap();
        assert!(result.is_empty(), "空输入应返回空结果");
    }
}

// ── Actor 进程操作 ──────────────────────────────────────────

mod start_process {
    use super::*;

    #[tokio::test]
    async fn returns_error_when_not_found() {
        let mgr = spawn_test_manager();
        let err = mgr.start_process("nonexistent").await.unwrap_err();
        assert!(
            err.to_string().contains("nonexistent"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod stop_process {
    use super::*;

    #[tokio::test]
    async fn returns_error_when_not_found() {
        let mgr = spawn_test_manager();
        let err = mgr.stop_process("nonexistent").await.unwrap_err();
        assert!(
            err.to_string().contains("nonexistent"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod restart_process {
    use super::*;

    #[tokio::test]
    async fn returns_error_when_not_found() {
        let mgr = spawn_test_manager();
        let err = mgr.restart_process("nonexistent").await.unwrap_err();
        assert!(
            err.to_string().contains("nonexistent"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod add_process {
    use super::*;

    #[tokio::test]
    async fn succeeds_with_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());

        mgr.add_process("test".to_string(), test_config(), false).await.unwrap();
        let info = mgr.get_process_info("test");
        assert!(info.is_some(), "添加后应能查询到进程 'test'");
    }

    #[tokio::test]
    async fn returns_error_on_duplicate_name() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("test".to_string(), test_config(), false).await.unwrap();

        let err = mgr.add_process("test".to_string(), test_config(), false).await.unwrap_err();
        assert!(
            err.to_string().contains("已存在"),
            "重复添加应报错已存在，实际: {err}"
        );
    }

    #[tokio::test]
    async fn returns_error_on_empty_command() {
        let mgr = spawn_test_manager();
        let mut pc = test_config();
        pc.command = "   ".to_string();

        let err = mgr.add_process("bad".to_string(), pc, false).await.unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "空命令应返回有意义的错误信息"
        );
    }
}

mod remove_process {
    use super::*;

    #[tokio::test]
    async fn succeeds_and_removes_from_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("test".to_string(), test_config(), false).await.unwrap();

        mgr.remove_process("test").await.unwrap();
        assert!(mgr.get_process_info("test").is_none(), "移除后不应再查询到");
    }

    #[tokio::test]
    async fn returns_error_when_not_found() {
        let mgr = spawn_test_manager();
        let err = mgr.remove_process("nonexistent").await.unwrap_err();
        assert!(
            err.to_string().contains("nonexistent"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

// ── Actor 快照读取 ──────────────────────────────────────────

mod list_processes {
    use super::*;

    #[tokio::test]
    async fn returns_all_added_processes() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());

        for name in &["a", "b", "c"] {
            mgr.add_process(name.to_string(), test_config(), false).await.unwrap();
        }

        let list = mgr.list_processes();
        assert_eq!(list.len(), 3, "应有 3 个进程，实际: {}", list.len());
    }
}

mod all_process_names {
    use super::*;

    #[tokio::test]
    async fn returns_names_of_all_processes() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("x".to_string(), test_config(), false).await.unwrap();
        mgr.add_process("y".to_string(), test_config(), false).await.unwrap();

        let names = mgr.all_process_names();
        assert_eq!(names.len(), 2, "应有 2 个进程名");
        assert!(names.contains(&"x".to_string()), "应包含 'x'");
        assert!(names.contains(&"y".to_string()), "应包含 'y'");
    }
}

mod snapshot {
    use super::*;

    #[tokio::test]
    async fn reflects_added_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        assert!(mgr.list_processes().is_empty(), "初始应为空");

        mgr.add_process("snap".to_string(), test_config(), false).await.unwrap();

        let list = mgr.list_processes();
        assert_eq!(list.len(), 1, "添加后应有 1 个进程");
        assert_eq!(list[0].name, "snap", "进程名应为 'snap'");
    }

    #[tokio::test]
    async fn reflects_removed_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("rm".to_string(), test_config(), false).await.unwrap();
        assert!(mgr.get_process_info("rm").is_some());

        mgr.remove_process("rm").await.unwrap();
        assert!(mgr.get_process_info("rm").is_none(), "移除后快照应为 None");
        assert!(mgr.list_processes().is_empty(), "移除后列表应为空");
    }
}

// ── Actor 配置与日志 ──────────────────────────────────────────

mod get_process_config {
    use super::*;

    #[tokio::test]
    async fn returns_config_for_existing_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        let mut cfg = test_config();
        cfg.command = "python".to_string();
        mgr.add_process("cfg".to_string(), cfg, false).await.unwrap();

        let fetched = mgr.get_process_config("cfg").await.unwrap().unwrap();
        assert_eq!(fetched.command, "python", "命令应为 'python'");
    }

    #[tokio::test]
    async fn returns_none_for_missing_process() {
        let mgr = spawn_test_manager();
        let result = mgr.get_process_config("missing").await.unwrap();
        assert!(result.is_none(), "不存在的进程应返回 None");
    }
}

mod clear_process_logs {
    use super::*;

    #[tokio::test]
    async fn succeeds_for_existing_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("logs".to_string(), test_config(), false).await.unwrap();

        mgr.clear_process_logs("logs").await.unwrap();
    }

    #[tokio::test]
    async fn returns_error_for_nonexistent() {
        let mgr = spawn_test_manager();
        let err = mgr.clear_process_logs("nonexistent").await.unwrap_err();
        assert!(
            err.to_string().contains("nonexistent"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod get_process_logs {
    use super::*;

    #[tokio::test]
    async fn returns_error_for_nonexistent() {
        let mgr = spawn_test_manager();
        let err = mgr.get_process_logs("nope", 100).await.unwrap_err();
        assert!(
            err.to_string().contains("nope"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod subscribe_process_logs {
    use super::*;

    #[tokio::test]
    async fn returns_error_for_nonexistent() {
        let mgr = spawn_test_manager();
        let err = mgr.subscribe_process_logs("nope").await.unwrap_err();
        assert!(
            err.to_string().contains("nope"),
            "错误应包含进程名，实际: {err}"
        );
    }
}

mod check_process_health {
    use super::*;

    #[tokio::test]
    async fn returns_error_when_no_health_config() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("svc".to_string(), test_config(), false).await.unwrap();

        let err = mgr.check_process_health("svc").await.unwrap_err();
        assert!(
            err.to_string().contains("健康检查"),
            "应提及未配置健康检查，实际: {err}"
        );
    }
}

mod reload_from_file {
    use super::*;

    #[tokio::test]
    async fn returns_error_when_no_config_path() {
        let mgr = spawn_test_manager();
        let err = mgr.reload_from_file().await.unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "无配置路径时应返回有意义的错误"
        );
    }

    #[tokio::test]
    async fn reloads_processes_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("orig".to_string(), test_config(), false).await.unwrap();

        let new_toml = r#"
[processes.fresh]
command = "echo fresh"
auto_start = false
"#;
        let config_path = dir.path().join("gugu.toml");
        std::fs::write(&config_path, new_toml).unwrap();

        mgr.reload_from_file().await.unwrap();
        assert!(mgr.get_process_info("orig").is_none(), "旧进程应被移除");
        assert!(mgr.get_process_info("fresh").is_some(), "新进程应被添加");
    }
}

mod reload_config {
    use super::*;

    #[tokio::test]
    async fn replaces_all_processes() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("old".to_string(), test_config(), false).await.unwrap();

        let mut new_config = AppConfig::default();
        let mut cfg = test_config();
        cfg.command = "new_cmd".to_string();
        new_config.processes.insert("new_svc".to_string(), cfg);

        mgr.reload_config(&new_config).await.unwrap();
        assert!(mgr.get_process_info("old").is_none(), "旧进程应被移除");
        assert!(mgr.get_process_info("new_svc").is_some(), "新进程应被添加");
    }
}

mod update_process {
    use super::*;

    #[tokio::test]
    async fn updates_config_for_existing_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("upd".to_string(), test_config(), false).await.unwrap();

        let mut new_cfg = test_config();
        new_cfg.command = "updated_cmd".to_string();
        mgr.update_process("upd", new_cfg, None, false).await.unwrap();

        let info = mgr.get_process_info("upd").unwrap();
        assert_eq!(info.command, "updated_cmd", "命令应已更新");
    }

    #[tokio::test]
    async fn returns_error_for_nonexistent() {
        let mgr = spawn_test_manager();
        let err = mgr.update_process("nope", test_config(), None, false).await.unwrap_err();
        assert!(
            err.to_string().contains("nope"),
            "错误应包含进程名，实际: {err}"
        );
    }

    #[tokio::test]
    async fn renames_process() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("old_name".to_string(), test_config(), false).await.unwrap();

        mgr.update_process("old_name", test_config(), Some("new_name".to_string()), false).await.unwrap();

        assert!(mgr.get_process_info("old_name").is_none(), "旧名应不存在");
        assert!(mgr.get_process_info("new_name").is_some(), "新名应存在");
    }

    #[tokio::test]
    async fn rejects_rename_to_existing_name() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("a".to_string(), test_config(), false).await.unwrap();
        mgr.add_process("b".to_string(), test_config(), false).await.unwrap();

        let err = mgr.update_process("a", test_config(), Some("b".to_string()), false).await.unwrap_err();
        assert!(
            err.to_string().contains("已存在"),
            "应报已存在，实际: {err}"
        );
    }
}

mod shutdown {
    use super::*;

    #[tokio::test]
    async fn stops_actor_and_rejects_subsequent_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = spawn_test_manager_with_path(dir.path());
        mgr.add_process("svc".to_string(), test_config(), false).await.unwrap();
        assert!(mgr.get_process_info("svc").is_some());

        mgr.shutdown();

        let err = mgr.start_process("svc").await.unwrap_err();
        assert!(
            err.to_string().contains("进程管理器"),
            "关闭后应拒绝命令，实际: {err}"
        );
    }
}
