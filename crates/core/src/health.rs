use crate::config::HealthCheckConfig;
use std::time::Duration;

pub async fn check_health(config: &HealthCheckConfig) -> bool {
    let timeout = Duration::from_secs(config.timeout_secs);
    match &config.check_type {
        crate::config::HealthCheckType::Tcp { port } => {
            let addr = format!("127.0.0.1:{port}");
            tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false)
        }
        crate::config::HealthCheckType::Http { url } => {
            reqwest::Client::new()
                .get(url)
                .timeout(timeout)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        }
    }
}
