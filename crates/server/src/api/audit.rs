//! 审计查询：按条件读取操作审计记录（新的在前）。

use axum::extract::{Query, State};
use axum::Json;
use chrono::{Duration, Utc};
use gugu_core::audit::{AuditEntry, AuditFilter, RETENTION_DAYS};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Default, Deserialize)]
pub(super) struct AuditQuery {
    /// 返回条数，默认 100，上限 500。
    limit: Option<usize>,
    /// 动作码，如 `process.start`；空串视为不筛选。
    action: Option<String>,
    /// 只看成功或只看失败。
    ok: Option<bool>,
    /// 关键字（对象、路径、来源、错误原因）。
    q: Option<String>,
    /// 只看最近 N 分钟。
    since_minutes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(super) struct AuditResponse {
    pub entries: Vec<AuditEntry>,
    pub retention_days: i64,
    /// 审计文件位置，方便排查「记录去哪了」。
    pub file: Option<String>,
}

pub(super) async fn list_audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Json<AuditResponse> {
    let filter = AuditFilter {
        action: query
            .action
            .map(|action| action.trim().to_string())
            .filter(|action| !action.is_empty()),
        only_failed: query.ok == Some(false),
        only_succeeded: query.ok == Some(true),
        since: query
            .since_minutes
            .filter(|minutes| *minutes > 0)
            .map(|minutes| Utc::now() - Duration::minutes(minutes as i64)),
        query: query.q,
        limit: query.limit.unwrap_or(0),
    };
    let entries = state.audit.query(&filter);
    let file = state
        .audit
        .file_path()
        .map(|path| path.display().to_string());
    Json(AuditResponse {
        entries,
        retention_days: RETENTION_DAYS,
        file,
    })
}
