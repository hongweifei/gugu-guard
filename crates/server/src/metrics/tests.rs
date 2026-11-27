//! 渲染测试：聚合行、每进程族、空数据与标签转义。

use gugu_core::history::TrafficPoint;
use gugu_core::process::{ProcessInfo, ProcessStatus};

use super::render::exposition;

fn info(name: &str, status: ProcessStatus) -> ProcessInfo {
    ProcessInfo {
        name: name.to_string(),
        command: "cmd".to_string(),
        args: Vec::new(),
        status,
        pid: None,
        restart_count: 2,
        auto_start: false,
        auto_restart: false,
        has_health_check: false,
        unhealthy_restart: false,
        healthy: None,
        group: None,
        started_at: None,
        uptime_secs: Some(7),
        recent_exits: Vec::new(),
        usage: None,
        listen_ports: Vec::new(),
        connections: 0,
    }
}

#[test]
fn renders_aggregates_and_per_process_lines() {
    let text = exposition(
        &[
            info("b", ProcessStatus::Stopped),
            info("a", ProcessStatus::Running),
        ],
        None,
    );
    assert!(text.contains("# TYPE gugu_processes gauge"), "{text}");
    assert!(text.contains("gugu_processes 2"), "{text}");
    assert!(text.contains("gugu_processes_running 1"), "{text}");
    assert!(text.contains("gugu_processes_stopped 1"), "{text}");
    assert!(text.contains("gugu_processes_failed 0"), "{text}");
    assert!(
        text.contains("gugu_process_status{process=\"a\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("gugu_process_status{process=\"b\"} 0"),
        "{text}"
    );
    assert!(
        text.contains("gugu_process_uptime_seconds{process=\"a\"} 7"),
        "{text}"
    );
    assert!(
        text.contains("gugu_process_restarts_total{process=\"a\"} 2"),
        "{text}"
    );
    // 按名字排序：a 在 b 之前
    assert!(text.find("process=\"a\"").unwrap() < text.find("process=\"b\"").unwrap());
    // 无健康数据时不输出该族
    assert!(!text.contains("gugu_process_health_status"), "{text}");
}

#[test]
fn renders_resource_usage_families() {
    let mut p = info("svc", ProcessStatus::Running);
    p.usage = Some(gugu_core::process::UsageSample {
        cpu_percent: 12.5,
        memory_bytes: 1024 * 1024,
        process_count: 2,
    });
    let text = exposition(&[p], None);
    assert!(
        text.contains("gugu_process_cpu_percent{process=\"svc\"} 12.50"),
        "{text}"
    );
    assert!(
        text.contains("gugu_process_memory_bytes{process=\"svc\"} 1048576"),
        "{text}"
    );
    // 没有采样数据时不输出这两族
    let text = exposition(&[info("idle", ProcessStatus::Stopped)], None);
    assert!(!text.contains("gugu_process_cpu_percent"), "{text}");
    assert!(!text.contains("gugu_process_memory_bytes"), "{text}");
}

#[test]
fn omits_per_process_families_when_empty_and_escapes_labels() {
    let text = exposition(&[], None);
    assert!(text.contains("gugu_processes 0"), "{text}");
    assert!(!text.contains("gugu_process_status"), "{text}");

    let mut p = info("with\"quote\\and\nnewline", ProcessStatus::Running);
    p.healthy = Some(false);
    let text = exposition(&[p], None);
    assert!(!text.contains("with\"quote"), "双引号必须被转义: {text}");
    assert!(text.contains("with\\\"quote\\\\and\\nnewline"), "{text}");
    assert!(
        text.contains("gugu_process_health_status{process=\"with\\\"quote\\\\and\\nnewline\"} 0"),
        "{text}"
    );
}

#[test]
fn renders_connection_and_net_families() {
    let mut p = info("web", ProcessStatus::Running);
    p.listen_ports = vec![8080];
    p.connections = 12;
    let net = TrafficPoint {
        at: chrono::Utc::now(),
        rx_bytes_per_sec: 2048.5,
        tx_bytes_per_sec: 512.0,
    };
    let text = exposition(&[p], Some(&net));
    assert!(
        text.contains("gugu_process_connections{process=\"web\"} 12"),
        "{text}"
    );
    assert!(
        text.contains("gugu_system_net_rx_bytes_per_sec 2048.50"),
        "{text}"
    );
    assert!(
        text.contains("gugu_system_net_tx_bytes_per_sec 512.00"),
        "{text}"
    );

    // 没有监听端口 / 没有带宽采样时不输出对应族
    let text = exposition(&[info("idle", ProcessStatus::Stopped)], None);
    assert!(!text.contains("gugu_process_connections"), "{text}");
    assert!(!text.contains("gugu_system_net_"), "{text}");
}
