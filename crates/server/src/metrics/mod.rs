//! `/metrics` 端点：自拼 Prometheus 文本格式（0.0.4）。
//!
//! 不引 `prometheus` crate —— 它经 protobuf 带来一整套只用到零头的传递依赖
//! （protobuf / protobuf-support / thiserror 1 …），而我们只用几个 gauge，
//! 文本格式本身是稳定且几十行就能写对的。
//!
//! 子模块：[`render`] 负责指标族拼装，测试在 [`tests`]。

use std::sync::Mutex;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use gugu_core::history::TrafficPoint;
use gugu_core::process::ProcessInfo;

use crate::state::AppState;

mod render;
#[cfg(test)]
mod tests;

/// 最近一次快照（update 时由 WebSocket 状态循环写入，render 时拼文本）。
#[derive(Default)]
pub struct Metrics {
    last: Mutex<Vec<ProcessInfo>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            last: Mutex::new(Vec::new()),
        }
    }

    pub fn update(&self, processes: &[ProcessInfo]) {
        *self.last.lock().unwrap() = processes.to_vec();
    }

    pub fn render(&self, net: Option<&TrafficPoint>) -> String {
        let processes = self.last.lock().unwrap();
        render::exposition(&processes, net)
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let net = state.manager.history().latest_traffic();
    state.metrics.update(&processes);
    let output = state.metrics.render(net.as_ref());
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}
