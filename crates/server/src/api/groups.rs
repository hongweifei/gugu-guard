//! 进程组：按 `group` 字段聚合的批量启动/停止/重启。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use gugu_core::process::ProcessInfo;

use crate::state::AppState;

use super::types::{ApiError, BatchResult, BatchResponse, GroupInfo};

fn group_members<'a>(processes: &'a [ProcessInfo], group: &str) -> Vec<&'a ProcessInfo> {
    processes.iter().filter(|p| p.group.as_deref() == Some(group)).collect()
}

/// 组不存在（无成员）时的统一响应。
fn group_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError::new("进程组未找到")),
    )
        .into_response()
}

pub(super) async fn list_groups(State(state): State<AppState>) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for p in &processes {
        if let Some(ref g) = p.group {
            groups.entry(g.clone()).or_default().push(p.name.clone());
        }
    }
    let result: Vec<GroupInfo> = groups
        .into_iter()
        .map(|(name, processes)| GroupInfo::new(name, processes))
        .collect();
    Json(result)
}

pub(super) async fn get_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> Response {
    let processes = state.manager.list_processes();
    let members = group_members(&processes, &group);
    if members.is_empty() {
        return group_not_found();
    }
    let names: Vec<String> = members.iter().map(|p| p.name.clone()).collect();
    Json(GroupInfo::new(group, names)).into_response()
}

pub(super) async fn start_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> Response {
    let processes = state.manager.list_processes();
    let members: Vec<String> = group_members(&processes, &group).iter().map(|p| p.name.clone()).collect();
    if members.is_empty() {
        return group_not_found();
    }
    let mut results = Vec::with_capacity(members.len());
    for name in members {
        let outcome = state.manager.start_process(&name).await;
        results.push(BatchResult::new(name, outcome));
    }
    Json(BatchResponse::new(format!("进程组 '{group}' 启动完成"), results)).into_response()
}

pub(super) async fn stop_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> Response {
    let processes = state.manager.list_processes();
    // 停止按成员逆序，与全局 stop_all 的依赖逆序语义一致
    let mut members: Vec<String> = group_members(&processes, &group).iter().map(|p| p.name.clone()).collect();
    if members.is_empty() {
        return group_not_found();
    }
    members.reverse();
    let mut results = Vec::with_capacity(members.len());
    for name in members {
        let outcome = state.manager.stop_process(&name).await;
        results.push(BatchResult::new(name, outcome));
    }
    Json(BatchResponse::new(format!("进程组 '{group}' 停止完成"), results)).into_response()
}

pub(super) async fn restart_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> Response {
    let processes = state.manager.list_processes();
    let members: Vec<String> = group_members(&processes, &group).iter().map(|p| p.name.clone()).collect();
    if members.is_empty() {
        return group_not_found();
    }
    let mut results = Vec::with_capacity(members.len());
    for name in members {
        let outcome = state.manager.restart_process(&name).await;
        results.push(BatchResult::new(name, outcome));
    }
    Json(BatchResponse::new(format!("进程组 '{group}' 重启完成"), results)).into_response()
}
