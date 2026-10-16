//! `/metrics` 端点：自拼 Prometheus 文本格式（0.0.4）。
//!
//! 不引 `prometheus` crate —— 它经 protobuf 带来一整套只用到零头的传递依赖
//! （protobuf / protobuf-support / thiserror 1 …），而我们只用几个 gauge，
//! 文本格式本身是稳定且几十行就能写对的。

use std::sync::Mutex;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use gugu_core::process::{ProcessInfo, ProcessStatus};

use crate::state::AppState;

/// 最近一次快照（update 时由 WebSocket 状态循环写入，render 时拼文本）。
#[derive(Default)]
pub struct Metrics {
    last: Mutex<Vec<ProcessInfo>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self { last: Mutex::new(Vec::new()) }
    }

    pub fn update(&self, processes: &[ProcessInfo]) {
        *self.last.lock().unwrap() = processes.to_vec();
    }

    pub fn render(&self) -> String {
        let processes = self.last.lock().unwrap();
        render_exposition(&processes)
    }
}

fn status_value(status: &ProcessStatus) -> i64 {
    match status {
        ProcessStatus::Running => 1,
        ProcessStatus::Starting => 2,
        ProcessStatus::Restarting => 3,
        ProcessStatus::Failed(_) => 4,
        ProcessStatus::Stopped | _ => 0,
    }
}

/// Prometheus 标签值转义：反斜杠、双引号、换行。
fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn header(out: &mut String, name: &str, help: &str) {
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
}

/// 输出按指标族名字典序，且与旧实现一致：无样本的族不输出。
fn render_exposition(processes: &[ProcessInfo]) -> String {
    let mut sorted: Vec<&ProcessInfo> = processes.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut running = 0i64;
    let mut stopped = 0i64;
    let mut failed = 0i64;
    for p in &sorted {
        match &p.status {
            ProcessStatus::Running => running += 1,
            ProcessStatus::Stopped => stopped += 1,
            ProcessStatus::Failed(_) => failed += 1,
            _ => {}
        }
    }

    let mut out = String::with_capacity(1024);

    if sorted.iter().any(|p| p.healthy.is_some()) {
        header(
            &mut out,
            "gugu_process_health_status",
            "Health check result: 1=healthy, 0=unhealthy",
        );
        for p in &sorted {
            if let Some(healthy) = p.healthy {
                out.push_str(&format!(
                    "gugu_process_health_status{{process=\"{}\"}} {}\n",
                    escape_label(&p.name),
                    i64::from(healthy)
                ));
            }
        }
    }

    if !sorted.is_empty() {
        header(
            &mut out,
            "gugu_process_restarts_total",
            "Number of crash restarts for the process",
        );
        for p in &sorted {
            out.push_str(&format!(
                "gugu_process_restarts_total{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                p.restart_count
            ));
        }

        header(
            &mut out,
            "gugu_process_status",
            "Process status: 1=running, 0=stopped, 2=starting, 3=restarting, 4=failed",
        );
        for p in &sorted {
            out.push_str(&format!(
                "gugu_process_status{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                status_value(&p.status)
            ));
        }

        header(
            &mut out,
            "gugu_process_uptime_seconds",
            "Process uptime in seconds",
        );
        for p in &sorted {
            out.push_str(&format!(
                "gugu_process_uptime_seconds{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                p.uptime_secs.unwrap_or(0)
            ));
        }
    }

    header(&mut out, "gugu_processes", "Total number of managed processes");
    out.push_str(&format!("gugu_processes {}\n", sorted.len()));

    header(&mut out, "gugu_processes_failed", "Number of failed processes");
    out.push_str(&format!("gugu_processes_failed {failed}\n"));

    header(&mut out, "gugu_processes_running", "Number of running processes");
    out.push_str(&format!("gugu_processes_running {running}\n"));

    header(&mut out, "gugu_processes_stopped", "Number of stopped processes");
    out.push_str(&format!("gugu_processes_stopped {stopped}\n"));

    out
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    state.metrics.update(&processes);
    let output = state.metrics.render();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        output,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn renders_aggregates_and_per_process_lines() {
        let text = render_exposition(&[
            info("b", ProcessStatus::Stopped),
            info("a", ProcessStatus::Running),
        ]);
        assert!(text.contains("# TYPE gugu_processes gauge"), "{text}");
        assert!(text.contains("gugu_processes 2"), "{text}");
        assert!(text.contains("gugu_processes_running 1"), "{text}");
        assert!(text.contains("gugu_processes_stopped 1"), "{text}");
        assert!(text.contains("gugu_processes_failed 0"), "{text}");
        assert!(text.contains("gugu_process_status{process=\"a\"} 1"), "{text}");
        assert!(text.contains("gugu_process_status{process=\"b\"} 0"), "{text}");
        assert!(text.contains("gugu_process_uptime_seconds{process=\"a\"} 7"), "{text}");
        assert!(text.contains("gugu_process_restarts_total{process=\"a\"} 2"), "{text}");
        // 按名字排序：a 在 b 之前
        assert!(text.find("process=\"a\"").unwrap() < text.find("process=\"b\"").unwrap());
        // 无健康数据时不输出该族
        assert!(!text.contains("gugu_process_health_status"), "{text}");
    }

    #[test]
    fn omits_per_process_families_when_empty_and_escapes_labels() {
        let text = render_exposition(&[]);
        assert!(text.contains("gugu_processes 0"), "{text}");
        assert!(!text.contains("gugu_process_status"), "{text}");

        let mut p = info("with\"quote\\and\nnewline", ProcessStatus::Running);
        p.healthy = Some(false);
        let text = render_exposition(&[p]);
        assert!(!text.contains("with\"quote"), "双引号必须被转义: {text}");
        assert!(text.contains("with\\\"quote\\\\and\\nnewline"), "{text}");
        assert!(text.contains("gugu_process_health_status{process=\"with\\\"quote\\\\and\\nnewline\"} 0"), "{text}");
    }
}
