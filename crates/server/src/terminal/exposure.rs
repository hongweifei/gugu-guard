//! 终端的启动门槛：等于把 shell 交给任何能打开面板的人，所以对外监听必须有 Key。

use std::net::IpAddr;

/// CLI 与库调用方启动前的安全校验：开启终端、监听非回环、又没有 API Key 时拒绝启动。
///
/// # Errors
/// 返回人类可读的拒绝原因，调用方直接把它作为启动失败信息。
pub fn check_exposure(addr: IpAddr, api_key: Option<&str>) -> Result<(), String> {
    let has_key = api_key.is_some_and(|key| !key.trim().is_empty());
    if addr.is_loopback() || has_key {
        return Ok(());
    }
    Err(format!(
        "已启用 WebUI 终端（daemon.web.terminal = true），但监听地址 {addr} 不是回环地址且未配置 API Key。\
         终端等于把 shell 交给任何能访问面板的人：请配置 daemon.api_key（或 GUGU_API_KEY），或只监听 127.0.0.1。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only_addresses_are_trusted() {
        for addr in ["127.0.0.1", "127.0.0.2", "::1"] {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(ip.is_loopback(), "{addr} 应视为回环");
            assert!(check_exposure(ip, None).is_ok());
        }
    }

    #[test]
    fn public_bind_without_key_is_rejected() {
        for addr in ["0.0.0.0", "192.168.1.10", "::"] {
            let ip: IpAddr = addr.parse().unwrap();
            let error = check_exposure(ip, None).unwrap_err();
            assert!(error.contains("api_key"), "{error}");
            let error = check_exposure(ip, Some("   ")).unwrap_err();
            assert!(error.contains("api_key"), "{error}");
        }
    }

    #[test]
    fn public_bind_with_key_is_allowed() {
        let ip: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(check_exposure(ip, Some("secret")).is_ok());
    }
}
