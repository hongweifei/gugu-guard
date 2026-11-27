//! `/proc/net/tcp{,6}` 行解析：纯文本逻辑，与平台无关，因此 Windows 上也能跑单测。

/// `/proc/net/tcp` 里的连接状态码（字段 `st`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Listen,
    Established,
    Other,
}

impl TcpState {
    #[must_use]
    pub fn from_hex(value: &str) -> Self {
        match value {
            "0A" | "0a" => Self::Listen,
            "01" => Self::Established,
            _ => Self::Other,
        }
    }
}

/// 解析 `/proc/net/tcp{,6}` 的一行，返回 (本地端口, 状态, inode)。
///
/// 行格式：`sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode`，
/// 其中 `local_address` 是 `HEXADDR:HEXPORT`（IPv6 是 32 位十六进制地址）。
/// 表头行与格式不符的行返回 None。
#[must_use]
pub fn port_from_line(line: &str) -> Option<(u16, TcpState, Option<u64>)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let local = *fields.get(1)?;
    let state = TcpState::from_hex(fields.get(3)?);
    let (_, port) = local.rsplit_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    // 第 10 列是 inode：拿不到就退化为 None，供上层跳过归属反查。
    let inode = fields.get(9).and_then(|v| v.parse::<u64>().ok());
    Some((port, state, inode))
}
