//! 终端 WebSocket 的同源校验。
//!
//! WebSocket 不受浏览器同源策略约束：不校验 `Origin`，任何网页都能连上本机
//! 终端。这里按「`Origin` 与 `Host` 完全一致」放行，并在没配 API Key 时额外要求
//! `Host` 本身指向本机，挡住 DNS rebinding。

use axum::http::HeaderMap;
use gugu_core::net::{authority_host, host_is_loopback};

/// 没有 `Origin` 的客户端（CLI、脚本）不受同源限制，但仍受 API Key 认证约束。
pub(super) fn origin_allowed(headers: &HeaderMap, has_key: bool) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get("host").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(origin_host) = origin_authority(origin) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if origin_host != host {
        return false;
    }
    has_key || is_loopback_host(&host)
}

/// 从 `http://host:port` 取出小写 `host:port`；带路径或 `null` 的来源一律拒绝。
fn origin_authority(origin: &str) -> Option<String> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    if rest.contains('/') || rest.is_empty() {
        return None;
    }
    Some(rest.to_ascii_lowercase())
}

/// Host 是否只指向本机（含端口）；IPv6 形如 `[::1]:9090`。
fn is_loopback_host(host: &str) -> bool {
    authority_host(host).is_some_and(|host| host_is_loopback(&host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(origin: Option<&'static str>, host: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert("origin", origin.parse().unwrap());
        }
        headers.insert("host", host.parse().unwrap());
        headers
    }

    #[test]
    fn websocket_origin_must_match_host() {
        assert!(
            origin_allowed(&headers(None, "127.0.0.1:9090"), false),
            "脚本客户端无 Origin"
        );
        assert!(origin_allowed(
            &headers(Some("http://127.0.0.1:9090"), "127.0.0.1:9090"),
            false
        ));
        assert!(origin_allowed(
            &headers(Some("http://localhost:9090"), "localhost:9090"),
            false
        ));
        assert!(origin_allowed(
            &headers(Some("http://[::1]:9090"), "[::1]:9090"),
            false
        ));

        assert!(!origin_allowed(
            &headers(Some("http://evil.example"), "127.0.0.1:9090"),
            false
        ));
        assert!(
            !origin_allowed(
                &headers(Some("https://gugu.example"), "gugu.example"),
                false
            ),
            "无 Key 时 DNS rebinding 域名必须被挡住"
        );
        assert!(
            origin_allowed(&headers(Some("https://gugu.example"), "gugu.example"), true),
            "有 Key 时允许非回环域名同源访问"
        );
        assert!(!origin_allowed(
            &headers(Some("null"), "127.0.0.1:9090"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("http://127.0.0.1:9090/path"), "127.0.0.1:9090"),
            true
        ));
        assert!(!origin_allowed(
            &headers(Some("http://192.168.1.5:9090"), "192.168.1.5:9090"),
            false
        ));
        assert!(origin_allowed(
            &headers(Some("http://192.168.1.5:9090"), "192.168.1.5:9090"),
            true
        ));
    }

    #[test]
    fn port_must_match_exactly() {
        assert!(!origin_allowed(
            &headers(Some("http://127.0.0.1:9091"), "127.0.0.1:9090"),
            true
        ));
        assert!(is_loopback_host("[::1]:9090"));
        assert!(is_loopback_host("LOCALHOST:9090"));
        assert!(is_loopback_host("127.0.0.2:9090"));
        assert!(!is_loopback_host("10.0.0.12:9090"));
        assert!(!is_loopback_host("gugu.example:9090"));
    }
}
