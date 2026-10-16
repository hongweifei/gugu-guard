//! 崩溃 / 不健康 / 恢复 通知：把事件推给自定义机器人 webhook。
//!
//! 通知是**尽力而为**：失败只记日志（重试一次），绝不阻塞或影响守护主流程。

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::WebhookFormat;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// 进程退出且不会再被自动拉起。
pub const EVENT_DOWN: &str = "process_down";
/// 健康检查连续失败达到阈值。
pub const EVENT_UNHEALTHY: &str = "process_unhealthy";
/// 进程树内存超限，已按自动重启处理。
pub const EVENT_MEMORY_LIMIT: &str = "process_memory_limit";
/// 进程被守护进程重新拉起。
pub const EVENT_RECOVERED: &str = "process_recovered";

fn client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// 构造 webhook 载荷（纯函数，便于单测）。
#[must_use]
pub fn build_body(format: WebhookFormat, message: &str, event: &Value) -> Value {
    match format {
        WebhookFormat::Dingtalk => json!({ "msgtype": "text", "text": { "content": message } }),
        WebhookFormat::Feishu => json!({ "msg_type": "text", "content": { "text": message } }),
        WebhookFormat::Raw => event.clone(),
    }
}

/// 组装事件 JSON（raw 格式直接用它，其余格式取其中字段拼文案）。
#[must_use]
pub fn event(event_type: &str, process: &str, detail: &str) -> Value {
    json!({
        "event": event_type,
        "process": process,
        "detail": detail,
        "time": chrono::Utc::now().to_rfc3339(),
        "source": "gugu-guard",
    })
}

/// 投递通知：失败重试一次，仍失败只记 warn。
pub async fn send(url: &str, format: WebhookFormat, message: &str, event: &Value) {
    let body = build_body(format, message, event);
    for attempt in 1..=2 {
        match client().post(url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => tracing::warn!("通知发送失败（第 {attempt} 次）: HTTP {}", resp.status()),
            Err(e) => tracing::warn!("通知发送失败（第 {attempt} 次）: {e}"),
        }
        if attempt == 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// fire-and-forget：不阻塞监控循环。
pub fn spawn(url: String, format: WebhookFormat, message: String, event: Value) {
    tokio::spawn(async move {
        send(&url, format, &message, &event).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_dingtalk_and_wecom_shape() {
        let body = build_body(WebhookFormat::Dingtalk, "进程挂了", &json!({}));
        assert_eq!(body["msgtype"], "text");
        assert_eq!(body["text"]["content"], "进程挂了");
    }

    #[test]
    fn builds_feishu_shape() {
        let body = build_body(WebhookFormat::Feishu, "进程挂了", &json!({}));
        assert_eq!(body["msg_type"], "text");
        assert_eq!(body["content"]["text"], "进程挂了");
        assert!(body.get("msgtype").is_none(), "飞书不认 msgtype 字段");
    }

    #[test]
    fn raw_format_forwards_event_payload() {
        let ev = event(EVENT_DOWN, "web", "退出码 1");
        let body = build_body(WebhookFormat::Raw, "文案忽略", &ev);
        assert_eq!(body, ev, "raw 应原样转发事件 JSON");
        assert_eq!(body["event"], "process_down");
        assert_eq!(body["process"], "web");
        assert_eq!(body["source"], "gugu-guard");
        assert!(
            body["time"].as_str().is_some_and(|t| t.contains('T')),
            "应带 RFC3339 时间"
        );
    }

    #[test]
    fn default_format_is_dingtalk() {
        assert_eq!(WebhookFormat::default(), WebhookFormat::Dingtalk);
    }
}
