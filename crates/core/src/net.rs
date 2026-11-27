//! URL 与主机判断的小工具：配置校验、节点路径与查询串拼装共用一套规则。

/// 从 `http(s)://host[:port][/path]` 里取主机名：去掉 userinfo、端口与 IPv6 方括号。
///
/// 没有 scheme、或者主机名为空（如 `http://`）时返回 `None`。
#[must_use]
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split_once(']').map_or("", |(host, _)| host)
    } else {
        host.split(':').next().unwrap_or("")
    };
    (!host.is_empty()).then_some(host)
}

/// 主机是否为本机回环地址（`localhost`、`127.0.0.0/8`、`::1`）。
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// 把一段文本编成 URL 路径段或查询值：保留 ASCII 字母数字与 `-` `_`，其余字节逐个转义。
#[must_use]
pub fn encode_path_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 节点标识能否安全放进 URL 路径：合法时返回转义后的路径段。
///
/// 空串、`.`、`..` 一律拒绝。URL 解析器会把它们当成目录段：
/// `%2E%2E` 同样是 `..`，转义也拦不住，请求会从 `/api/v1/nodes/../processes`
/// 折叠成 `/api/v1/processes`，落到本机而不是从节点。
#[must_use]
pub fn node_path_segment(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return None;
    }
    Some(encode_path_segment(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_host_strips_userinfo_port_and_brackets() {
        assert_eq!(url_host("http://10.0.0.12:9090/api"), Some("10.0.0.12"));
        assert_eq!(url_host("http://[::1]:9090"), Some("::1"));
        assert_eq!(
            url_host("https://user:pw@example.com/x"),
            Some("example.com")
        );
        assert_eq!(url_host("http:///path"), None);
        assert_eq!(url_host("http://"), None);
        assert_eq!(url_host("example.com"), None);
    }

    #[test]
    fn loopback_detection_covers_ipv6_and_localhost() {
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("LOCALHOST"));
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("127.9.9.9"));
        assert!(host_is_loopback("::1"));
        assert!(!host_is_loopback("10.0.0.12"));
        assert!(!host_is_loopback("example.com"));
    }

    #[test]
    fn path_segments_never_keep_dot_segments() {
        assert_eq!(encode_path_segment("web1"), "web1");
        assert_eq!(encode_path_segment("a.b-c_9"), "a%2Eb-c_9");
        assert_eq!(encode_path_segment(".."), "%2E%2E");
        assert_eq!(encode_path_segment("a b+c"), "a%20b%2Bc");
        assert_eq!(encode_path_segment("边缘"), "%E8%BE%B9%E7%BC%98");
    }

    #[test]
    fn node_ids_fold_into_parent_directories_are_rejected() {
        assert_eq!(node_path_segment("web1").as_deref(), Some("web1"));
        assert_eq!(node_path_segment(" a.b ").as_deref(), Some("a%2Eb"));
        assert_eq!(node_path_segment(".."), None);
        assert_eq!(node_path_segment("."), None);
        assert_eq!(node_path_segment("   "), None);
        assert_eq!(node_path_segment("a/b"), None);
    }
}
