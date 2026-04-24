use std::sync::Mutex;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use gugu_core::process::{ProcessInfo, ProcessStatus};
use prometheus::{Encoder, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder};

use crate::state::AppState;

pub struct Metrics {
    registry: Registry,
    processes_total: IntGauge,
    processes_running: IntGauge,
    processes_stopped: IntGauge,
    processes_failed: IntGauge,
    process_status: IntGaugeVec,
    process_restarts: IntGaugeVec,
    process_uptime: IntGaugeVec,
    process_health: IntGaugeVec,
    prev_names: Mutex<Vec<String>>,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let processes_total = IntGauge::new(
            "gugu_processes",
            "Total number of managed processes",
        )
        .unwrap();
        let processes_running = IntGauge::new(
            "gugu_processes_running",
            "Number of running processes",
        )
        .unwrap();
        let processes_stopped = IntGauge::new(
            "gugu_processes_stopped",
            "Number of stopped processes",
        )
        .unwrap();
        let processes_failed = IntGauge::new(
            "gugu_processes_failed",
            "Number of failed processes",
        )
        .unwrap();
        let process_status = IntGaugeVec::new(
            Opts::new(
                "gugu_process_status",
                "Process status: 1=running, 0=stopped, 2=starting, 3=restarting, 4=failed",
            ),
            &["process"],
        )
        .unwrap();
        let process_restarts = IntGaugeVec::new(
            Opts::new(
                "gugu_process_restarts_total",
                "Number of crash restarts for the process",
            ),
            &["process"],
        )
        .unwrap();
        let process_uptime = IntGaugeVec::new(
            Opts::new(
                "gugu_process_uptime_seconds",
                "Process uptime in seconds",
            ),
            &["process"],
        )
        .unwrap();
        let process_health = IntGaugeVec::new(
            Opts::new(
                "gugu_process_health_status",
                "Health check result: 1=healthy, 0=unhealthy",
            ),
            &["process"],
        )
        .unwrap();

        registry
            .register(Box::new(processes_total.clone()))
            .unwrap();
        registry
            .register(Box::new(processes_running.clone()))
            .unwrap();
        registry
            .register(Box::new(processes_stopped.clone()))
            .unwrap();
        registry
            .register(Box::new(processes_failed.clone()))
            .unwrap();
        registry
            .register(Box::new(process_status.clone()))
            .unwrap();
        registry
            .register(Box::new(process_restarts.clone()))
            .unwrap();
        registry
            .register(Box::new(process_uptime.clone()))
            .unwrap();
        registry
            .register(Box::new(process_health.clone()))
            .unwrap();

        Self {
            registry,
            processes_total,
            processes_running,
            processes_stopped,
            processes_failed,
            process_status,
            process_restarts,
            process_uptime,
            process_health,
            prev_names: Mutex::new(Vec::new()),
        }
    }

    pub fn update(&self, processes: &[ProcessInfo]) {
        let current_names: Vec<String> = processes.iter().map(|p| p.name.clone()).collect();

        {
            let prev = self.prev_names.lock().unwrap();
            for name in prev.iter() {
                if !current_names.contains(name) {
                    let _ = self.process_status.remove_label_values(&[name]);
                    let _ = self.process_restarts.remove_label_values(&[name]);
                    let _ = self.process_uptime.remove_label_values(&[name]);
                    let _ = self.process_health.remove_label_values(&[name]);
                }
            }
        }

        let mut running = 0i64;
        let mut stopped = 0i64;
        let mut failed = 0i64;

        for p in processes {
            let status_val = match &p.status {
                ProcessStatus::Stopped => 0,
                ProcessStatus::Running => 1,
                ProcessStatus::Starting => 2,
                ProcessStatus::Restarting => 3,
                ProcessStatus::Failed(_) => 4,
                _ => 0,
            };

            self.process_status
                .with_label_values(&[&p.name])
                .set(status_val);
            self.process_restarts
                .with_label_values(&[&p.name])
                .set(p.restart_count as i64);
            self.process_uptime
                .with_label_values(&[&p.name])
                .set(p.uptime_secs.unwrap_or(0));

            if let Some(healthy) = p.healthy {
                self.process_health
                    .with_label_values(&[&p.name])
                    .set(if healthy { 1 } else { 0 });
            }

            match &p.status {
                ProcessStatus::Running => running += 1,
                ProcessStatus::Stopped => stopped += 1,
                ProcessStatus::Failed(_) => failed += 1,
                _ => {}
            }
        }

        self.processes_total.set(processes.len() as i64);
        self.processes_running.set(running);
        self.processes_stopped.set(stopped);
        self.processes_failed.set(failed);

        *self.prev_names.lock().unwrap() = current_names;
    }

    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
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
