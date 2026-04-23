use crate::config::ProcessConfig;
use crate::error::GuguError;
use crate::manager::topological_sort;
use std::collections::HashMap;

fn make_proc(name: &str, depends_on: Vec<&str>) -> (String, ProcessConfig) {
    (
        name.to_string(),
        ProcessConfig {
            command: format!("run_{name}"),
            depends_on: depends_on.into_iter().map(String::from).collect(),
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
            max_log_size_mb: None,
            stdout_log: None,
            stderr_log: None,
        },
    )
}

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

#[test]
fn topo_sort_no_deps() {
    let mut map = HashMap::new();
    let (k1, v1) = make_proc("a", vec![]);
    let (k2, v2) = make_proc("b", vec![]);
    let (k3, v3) = make_proc("c", vec![]);
    map.insert(k1, v1);
    map.insert(k2, v2);
    map.insert(k3, v3);

    let result = topological_sort(&map).unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn topo_sort_linear_chain() {
    let mut map = HashMap::new();
    let (k1, v1) = make_proc("a", vec![]);
    let (k2, v2) = make_proc("b", vec!["a"]);
    let (k3, v3) = make_proc("c", vec!["b"]);
    map.insert(k1, v1);
    map.insert(k2, v2);
    map.insert(k3, v3);

    let result = topological_sort(&map).unwrap();
    assert_eq!(result.len(), 3);
    assert!(result.iter().position(|n| n == "a").unwrap() < result.iter().position(|n| n == "b").unwrap());
    assert!(result.iter().position(|n| n == "b").unwrap() < result.iter().position(|n| n == "c").unwrap());
}

#[test]
fn topo_sort_diamond() {
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
    assert_eq!(result.len(), 4);
    let pos_a = result.iter().position(|n| n == "a").unwrap();
    let pos_b = result.iter().position(|n| n == "b").unwrap();
    let pos_c = result.iter().position(|n| n == "c").unwrap();
    let pos_d = result.iter().position(|n| n == "d").unwrap();
    assert!(pos_a < pos_b);
    assert!(pos_a < pos_c);
    assert!(pos_b < pos_d);
    assert!(pos_c < pos_d);
}

#[test]
fn topo_sort_cycle_detected() {
    let mut map = HashMap::new();
    let (k1, v1) = make_proc("a", vec!["b"]);
    let (k2, v2) = make_proc("b", vec!["c"]);
    let (k3, v3) = make_proc("c", vec!["a"]);
    map.insert(k1, v1);
    map.insert(k2, v2);
    map.insert(k3, v3);

    let result = topological_sort(&map);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("循环依赖"));
}

#[test]
fn topo_sort_missing_dependency_ignored() {
    let mut map = HashMap::new();
    let (k, v) = make_proc("a", vec!["nonexistent"]);
    map.insert(k, v);

    let result = topological_sort(&map).unwrap();
    assert_eq!(result, vec!["a"]);
}

#[test]
fn topo_sort_self_cycle() {
    let mut map = HashMap::new();
    let (k, v) = make_proc("a", vec!["a"]);
    map.insert(k, v);

    let result = topological_sort(&map);
    assert!(result.is_err());
}

#[test]
fn topo_sort_empty() {
    let map: HashMap<String, ProcessConfig> = HashMap::new();
    let result = topological_sort(&map).unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn manager_process_not_found() {
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, None);

    let result = mgr.start_process("nonexistent").await;
    assert!(matches!(result, Err(GuguError::ProcessNotFound(_))));

    let result = mgr.stop_process("nonexistent").await;
    assert!(matches!(result, Err(GuguError::ProcessNotFound(_))));

    let result = mgr.restart_process("nonexistent").await;
    assert!(matches!(result, Err(GuguError::ProcessNotFound(_))));
}

#[tokio::test]
async fn manager_add_remove_process() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, Some(config_path));

    mgr.add_process("test".to_string(), test_config(), false).await.unwrap();
    assert!(mgr.get_process_info("test").is_some());

    let result = mgr.add_process("test".to_string(), test_config(), false).await;
    assert!(result.is_err());

    mgr.remove_process("test").await.unwrap();
    assert!(mgr.get_process_info("test").is_none());
}

#[tokio::test]
async fn manager_add_process_empty_command() {
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, None);

    let mut pc = test_config();
    pc.command = "   ".to_string();
    let result = mgr.add_process("bad".to_string(), pc, false).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_list_processes() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, Some(config_path));

    for name in &["a", "b", "c"] {
        mgr.add_process(name.to_string(), test_config(), false).await.unwrap();
    }

    let list = mgr.list_processes();
    assert_eq!(list.len(), 3);
}

#[tokio::test]
async fn manager_check_health_no_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, Some(config_path));

    mgr.add_process("svc".to_string(), test_config(), false).await.unwrap();

    let result = mgr.check_process_health("svc").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_reload_from_file_no_path() {
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, None);
    let result = mgr.reload_from_file().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_subscribe_logs_nonexistent() {
    let config = crate::config::AppConfig::default();
    let mgr = crate::ProcessManager::new(&config, None);
    let result = mgr.subscribe_process_logs("nope");
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_get_logs_nonexistent() {
    let config = crate::config::AppConfig::default();
    let mgr = crate::ProcessManager::new(&config, None);
    let result = mgr.get_process_logs("nope", 100).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_all_process_names() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = crate::config::AppConfig::default();
    let mut mgr = crate::ProcessManager::new(&config, Some(config_path));

    for name in &["x", "y"] {
        mgr.add_process(name.to_string(), test_config(), false).await.unwrap();
    }

    let names = mgr.all_process_names();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"x".to_string()));
    assert!(names.contains(&"y".to_string()));
}
