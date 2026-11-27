//! CLI 客户端：连接守护进程 HTTP API，分发子命令并输出结果。
//!
//! 传输与输出分开：本文件只管请求（含 `--node` 的代理路径改写）与错误口径，
//! 表格、日志行与格式化在 [`output`]。

mod fs;
mod output;

use anyhow::{Context, Result};
use gugu_core::config::{AppConfig, DEFAULT_BIND_ADDR, DEFAULT_PORT, LOCALHOST};
use gugu_core::logs::{LogEncoding, LogSearchReport};
use gugu_core::net::{encode_path_segment, node_path_segment};
use gugu_core::process::LogEntry;
use std::collections::HashMap;
use std::path::Path;

use crate::Commands;

struct ApiHelper<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    key: Option<&'a str>,
    /// 已规范化的从节点标识；None 表示本机。
    node: Option<String>,
    /// 已转义的从节点路径段；None 表示直连本机守护进程。
    node_segment: Option<String>,
}

impl<'a> ApiHelper<'a> {
    /// `node` 为空、空白或 `local` 时直连本机；其余按路径段校验并转义后存下来，
    /// 非法标识在发请求之前就报错，不会静默落到本机接口上。
    fn new(
        client: &'a reqwest::Client,
        base: &'a str,
        key: Option<&'a str>,
        node: Option<&str>,
    ) -> Result<Self> {
        let node = node
            .map(str::trim)
            .filter(|node| !node.is_empty() && *node != gugu_core::config::LOCAL_NODE_NAME);
        let node_segment = match node {
            Some(name) => Some(node_path_segment(name).ok_or_else(|| {
                anyhow::anyhow!("节点标识 '{name}' 不能放进 URL 路径，检查 --node")
            })?),
            None => None,
        };
        Ok(Self {
            client,
            base,
            key,
            node: node.map(str::to_string),
            node_segment,
        })
    }

    /// 当前选中的节点标识；本机返回 None。
    fn node_name(&self) -> Option<&str> {
        self.node.as_deref()
    }

    /// 直连中心节点：跨节点搬运要在 body 里点名两端，不能走 `--node` 的代理前缀。
    fn direct(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.client.request(method, format!("{}{path}", self.base));
        match self.key {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        }
    }

    /// 实际请求地址：指定从节点时改走中心节点的代理路径 `/api/v1/nodes/{node}/...`。
    fn url(&self, path: &str) -> String {
        match &self.node_segment {
            Some(segment) => {
                let rest = path.strip_prefix("/api/v1").unwrap_or(path);
                format!("{}/api/v1/nodes/{segment}{rest}", self.base)
            }
            None => format!("{}{path}", self.base),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.client.request(method, self.url(path));
        match self.key {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::GET, path)
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::POST, path)
    }

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::DELETE, path)
    }
}

pub(crate) fn get_server_url(config_path: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(url) = explicit {
        return Ok(url.to_string());
    }
    let config = AppConfig::load(config_path).context("加载配置文件失败")?;
    let addr = config.daemon.web.addr.as_deref().unwrap_or(LOCALHOST);
    let port = config.daemon.web.port.unwrap_or(DEFAULT_PORT);
    let connect_addr = if addr == DEFAULT_BIND_ADDR {
        LOCALHOST
    } else {
        addr
    };
    Ok(format!("http://{connect_addr}:{port}"))
}

pub(crate) fn get_api_key(config_path: &Path, explicit: Option<&str>) -> Result<Option<String>> {
    if let Some(key) = explicit {
        return Ok(Some(key.to_string()));
    }
    let config = AppConfig::load(config_path).ok();
    Ok(config.and_then(|c| c.daemon.api_key))
}

pub(crate) async fn run_client(
    command: &Commands,
    server_url: &str,
    api_key: Option<&str>,
    node: Option<&str>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("gugu-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("初始化 HTTP 客户端失败")?;
    let api = ApiHelper::new(&client, server_url, api_key, node)?;

    match command {
        Commands::Nodes { json } => {
            // 节点表是中心节点自己的概念，不受 --node 影响。
            let hub = ApiHelper::new(&client, server_url, api_key, None)?;
            let body: output::NodesResponse = get_json(&hub, "/api/v1/nodes").await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                output::print_nodes_table(&body);
            }
        }

        Commands::Status { json } => {
            let processes: Vec<gugu_core::process::ProcessInfo> =
                get_json(&api, "/api/v1/processes").await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&processes)?);
            } else {
                output::print_status_table(&processes);
            }
        }

        Commands::List { json } => {
            let processes: Vec<gugu_core::process::ProcessInfo> =
                get_json(&api, "/api/v1/processes").await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&processes)?);
            } else {
                output::print_list_table(&processes);
            }
        }

        Commands::Start { name } => {
            let resp = api
                .post(&format!("/api/v1/processes/{name}/start"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Stop { name } => {
            let resp = api
                .post(&format!("/api/v1/processes/{name}/stop"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Restart { name } => {
            let resp = api
                .post(&format!("/api/v1/processes/{name}/restart"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Add { name, opts } => {
            let mut env_map = HashMap::new();
            for e in &opts.env {
                if let Some((k, v)) = e.split_once('=') {
                    env_map.insert(k.to_string(), v.to_string());
                }
            }
            let log_format = match opts.log_format.as_deref() {
                None => None,
                Some(raw) => Some(parse_log_format(raw)?),
            };
            let encoding = opts
                .encoding
                .as_deref()
                .map(|raw| raw.parse::<LogEncoding>().map_err(anyhow::Error::msg))
                .transpose()?;
            let body = serde_json::json!({
                "command": opts.command,
                "args": opts.args,
                "working_dir": opts.dir,
                "env": env_map,
                "auto_start": !opts.no_auto_start,
                "auto_restart": !opts.no_auto_restart,
                "max_restarts": opts.max_restarts,
                "restart_delay_secs": opts.restart_delay,
                "stop_timeout_secs": opts.stop_timeout,
                "stdout_log": opts.stdout_log,
                "stderr_log": opts.stderr_log,
                "log_format": log_format,
                "encoding": encoding,
                "cpu_max": opts.cpu_max,
                "memory_max": opts.memory_max,
                "pids_max": opts.pids_max,
                "listen_ports": opts.port,
                "port_check": !opts.no_port_check,
                "ready_timeout_secs": opts.ready_timeout,
                "start_now": opts.start,
            });
            let resp = api
                .post(&format!("/api/v1/processes/{name}"))
                .json(&body)
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Remove { name } => {
            let resp = api
                .delete(&format!("/api/v1/processes/{name}"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Logs {
            name,
            lines,
            level,
            query,
            stream,
            file,
            budget_mb,
            limit,
        } => {
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(raw) = level {
                params.push(("level", raw.clone()));
            }
            if let Some(raw) = query {
                params.push(("q", raw.clone()));
            }
            if let Some(raw) = stream {
                params.push(("stream", raw.clone()));
            }

            if *file {
                params.push(("limit", limit.to_string()));
                params.push(("budget_mb", budget_mb.to_string()));
                let url = format!(
                    "/api/v1/processes/{name}/logs/search?{}",
                    query_string(&params)
                );
                let report: LogSearchReport = get_json(&api, &url).await?;
                output::print_log_report(&report);
            } else {
                params.push(("lines", lines.to_string()));
                let url = format!("/api/v1/processes/{name}/logs?{}", query_string(&params));
                let logs: Vec<LogEntry> = get_json(&api, &url).await?;
                output::print_log_entries(&logs);
            }
        }

        Commands::History {
            name,
            minutes,
            json,
        } => {
            let body: output::HistoryResponse = get_json(
                &api,
                &format!("/api/v1/processes/{name}/history?minutes={minutes}"),
            )
            .await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                output::print_history_table(name, &body.points);
            }
        }

        Commands::Exits {
            name,
            minutes,
            limit,
            json,
        } => {
            let body: output::ExitsResponse = get_json(
                &api,
                &format!("/api/v1/processes/{name}/exits?minutes={minutes}&limit={limit}"),
            )
            .await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                output::print_exits(name, &body.events, &body.stats);
            }
        }

        Commands::Net { minutes, json } => {
            let body: output::SystemResponse =
                get_json(&api, &format!("/api/v1/system?minutes={minutes}")).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                output::print_net_table(&body.points, body.latest.as_ref());
            }
        }

        Commands::Audit {
            limit,
            action,
            failed,
            since,
            json,
        } => {
            let mut params: Vec<(&str, String)> = vec![("limit", limit.to_string())];
            if let Some(action) = action {
                params.push(("action", action.clone()));
            }
            if *failed {
                params.push(("ok", "false".to_string()));
            }
            if *since > 0 {
                params.push(("since_minutes", since.to_string()));
            }
            let body: output::AuditResponse =
                get_json(&api, &format!("/api/v1/audit?{}", query_string(&params))).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                output::print_audit(&body.entries, body.retention_days);
            }
        }

        Commands::Update {
            check,
            version,
            no_wait,
        } => {
            let resp = api.post("/api/v1/update/check").send().await?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(api_error(status, &text));
            }
            let snapshot: output::UpdateSnapshot =
                serde_json::from_str(&text).context("解析 /api/v1/update/check 的响应失败")?;
            output::print_update(&snapshot);
            if *check || !snapshot.available {
                return Ok(());
            }
            if !snapshot.can_apply {
                anyhow::bail!("当前环境无法一键更新，请到发布页手动下载");
            }

            let target = version.clone().or_else(|| {
                snapshot
                    .latest
                    .as_ref()
                    .map(|release| release.version.clone())
            });
            let resp = api
                .post("/api/v1/update/apply")
                .json(&serde_json::json!({ "version": target }))
                .send()
                .await?;
            print_api_response(resp).await?;
            if *no_wait {
                return Ok(());
            }
            wait_for_new_version(&api, &snapshot.current, target.as_deref()).await?;
        }

        Commands::Fs { action } => {
            fs::dispatch(&api, action).await?;
        }

        Commands::Reload => {
            let resp = api.post("/api/v1/reload").send().await?;
            print_api_response(resp).await?;
        }

        // main 只把客户端命令交给这里；本地命令在 main 里就地处理。
        Commands::Run
        | Commands::Install { .. }
        | Commands::Uninstall { .. }
        | Commands::Check
        | Commands::ApplyUpdate { .. }
        | Commands::Daemon { .. } => {
            anyhow::bail!("本地命令不应进入客户端分发")
        }
    }

    Ok(())
}

/// 更新后守护进程会自行重启：轮询到新版本上线，或一直连不上直到超时。
async fn wait_for_new_version(
    api: &ApiHelper<'_>,
    previous: &str,
    expected: Option<&str>,
) -> Result<()> {
    println!("正在等待守护进程重启...");
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let Ok(resp) = api.get("/api/v1/node").send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let version = body
            .get("version")
            .and_then(|version| version.as_str())
            .unwrap_or_default();
        let matched = match expected {
            Some(want) => version == want,
            None => !version.is_empty() && version != previous,
        };
        if matched {
            println!("更新完成，当前版本 {version}");
            return Ok(());
        }
    }
    anyhow::bail!(
        "等待超时：守护进程未在两分钟内以新版本上线，请查看 log_dir 与暂存目录的 apply.log"
    )
}

/// 取一个 JSON 接口的响应体；非 2xx 时把服务端的一句话错误带出来
/// （代理到从节点时这里会显示「节点 web1 不可达」这类原因，而不是解析失败）。
async fn get_json<T: serde::de::DeserializeOwned>(api: &ApiHelper<'_>, path: &str) -> Result<T> {
    let resp = api.get(path).send().await.context("无法连接到守护进程")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(api_error(status, &text));
    }
    serde_json::from_str(&text).with_context(|| format!("解析 {path} 的响应失败"))
}

/// 服务端错误响应统一取 `error` 字段；没有 JSON 就退回原文。
fn api_error(status: reqwest::StatusCode, text: &str) -> anyhow::Error {
    let message = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| text.trim().to_string());
    anyhow::anyhow!("请求失败 ({status}): {message}")
}

async fn print_api_response(resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(api_error(status, &text));
    }
    let body: serde_json::Value = resp.json().await?;
    if let Some(msg) = body.get("message").and_then(serde_json::Value::as_str) {
        println!("{msg}");
    } else if let Some(err) = body.get("error").and_then(serde_json::Value::as_str) {
        eprintln!("错误: {err}");
    }
    Ok(())
}

fn parse_log_format(raw: &str) -> Result<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok("auto"),
        "json" => Ok("json"),
        "logfmt" => Ok("logfmt"),
        "text" => Ok("text"),
        other => anyhow::bail!("未知日志格式 {other}，可选 auto / json / logfmt / text"),
    }
}

/// 拼查询串：值按路径段同一套规则转义。
fn query_string(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", encode_path_segment(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper<'a>(client: &'a reqwest::Client, node: Option<&'a str>) -> ApiHelper<'a> {
        ApiHelper::new(client, "http://hub:9090", Some("hub-secret"), node).expect("节点标识应合法")
    }

    #[test]
    fn local_node_requests_stay_on_the_hub_path() {
        let client = reqwest::Client::new();
        let api = helper(&client, None);
        assert_eq!(
            api.url("/api/v1/processes"),
            "http://hub:9090/api/v1/processes"
        );
        let api = helper(&client, Some("local"));
        assert_eq!(
            api.url("/api/v1/processes"),
            "http://hub:9090/api/v1/processes"
        );
    }

    #[test]
    fn remote_node_requests_use_the_proxy_prefix() {
        let client = reqwest::Client::new();
        let api = helper(&client, Some("web1"));
        assert_eq!(
            api.url("/api/v1/processes/api/start"),
            "http://hub:9090/api/v1/nodes/web1/processes/api/start"
        );
    }

    #[test]
    fn dotted_node_ids_are_rejected_before_the_request() {
        let client = reqwest::Client::new();
        for bad in ["..", ".", "a/b"] {
            let Err(err) = ApiHelper::new(&client, "http://hub:9090", None, Some(bad)) else {
                panic!("非法节点标识应在发请求前被拒绝: {bad}");
            };
            assert!(
                err.to_string().contains("URL 路径"),
                "{bad} 的错误应说明路径问题: {err}"
            );
        }
        // 空白标识与本机标识一样按「不指定节点」处理。
        assert_eq!(
            helper(&client, Some("  ")).url("/api/v1/status"),
            "http://hub:9090/api/v1/status"
        );
    }

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(
            query_string(&[("q", "a b".to_string())]),
            "q=a%20b".to_string()
        );
    }
}
