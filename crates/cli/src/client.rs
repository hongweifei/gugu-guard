//! CLI 客户端子命令：连接守护进程 HTTP API 并输出表格/文本。

use anyhow::{Context, Result};
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, Color, Table,
};
use gugu_core::config::{AppConfig, DEFAULT_BIND_ADDR, DEFAULT_PORT, LOCALHOST};
use gugu_core::history::{ExitEvent, ExitStats, MetricPoint, TrafficPoint};
use std::collections::HashMap;
use std::path::Path;

use crate::Commands;

struct ApiHelper<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    key: Option<&'a str>,
}

impl<'a> ApiHelper<'a> {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.client.get(format!("{}{path}", self.base));
        if let Some(key) = self.key {
            r = r.header("Authorization", format!("Bearer {key}"));
        }
        r
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.client.post(format!("{}{path}", self.base));
        if let Some(key) = self.key {
            r = r.header("Authorization", format!("Bearer {key}"));
        }
        r
    }

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.client.delete(format!("{}{path}", self.base));
        if let Some(key) = self.key {
            r = r.header("Authorization", format!("Bearer {key}"));
        }
        r
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
) -> Result<()> {
    let client = reqwest::Client::new();
    let api = ApiHelper {
        client: &client,
        base: server_url,
        key: api_key,
    };

    match command {
        Commands::Status { json } => {
            let resp = api
                .get("/api/v1/processes")
                .send()
                .await
                .context("无法连接到守护进程，请确认是否已启动")?;
            let processes: Vec<gugu_core::process::ProcessInfo> = resp.json().await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&processes)?);
            } else {
                print_status_table(&processes);
            }
        }

        Commands::List { json } => {
            let resp = api
                .get("/api/v1/processes")
                .send()
                .await
                .context("无法连接到守护进程")?;
            let processes: Vec<gugu_core::process::ProcessInfo> = resp.json().await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&processes)?);
            } else {
                print_list_table(&processes);
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

        Commands::Add {
            name,
            command,
            args,
            dir,
            env,
            no_auto_start,
            no_auto_restart,
            max_restarts,
            restart_delay,
            stop_timeout,
            stdout_log,
            stderr_log,
            port,
            no_port_check,
            ready_timeout,
            start,
        } => {
            let mut env_map = HashMap::new();
            for e in env {
                if let Some((k, v)) = e.split_once('=') {
                    env_map.insert(k.to_string(), v.to_string());
                }
            }
            let body = serde_json::json!({
                "command": command,
                "args": args,
                "working_dir": dir,
                "env": env_map,
                "auto_start": !no_auto_start,
                "auto_restart": !no_auto_restart,
                "max_restarts": max_restarts,
                "restart_delay_secs": restart_delay,
                "stop_timeout_secs": stop_timeout,
                "stdout_log": stdout_log,
                "stderr_log": stderr_log,
                "listen_ports": port,
                "port_check": !no_port_check,
                "ready_timeout_secs": ready_timeout,
                "start_now": start,
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

        Commands::Logs { name, lines } => {
            let resp = api
                .get(&format!("/api/v1/processes/{name}/logs?lines={lines}"))
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("获取日志失败: {text}");
            }
            let logs: Vec<gugu_core::process::LogEntry> = resp.json().await?;
            for entry in &logs {
                let time = entry.timestamp.format("%H:%M:%S");
                let prefix = match &entry.stream {
                    gugu_core::process::LogStream::Stdout => "OUT",
                    gugu_core::process::LogStream::Stderr => "ERR",
                    _ => "???",
                };
                println!("[{time}] [{prefix}] {}", entry.line);
            }
            if logs.is_empty() {
                println!("暂无日志");
            }
        }

        Commands::History {
            name,
            minutes,
            json,
        } => {
            let body = fetch_json(
                &api,
                &format!("/api/v1/processes/{name}/history?minutes={minutes}"),
            )
            .await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                let data: HistoryResponse = serde_json::from_value(body)?;
                print_history_table(name, &data.points);
            }
        }

        Commands::Exits {
            name,
            minutes,
            limit,
            json,
        } => {
            let body = fetch_json(
                &api,
                &format!("/api/v1/processes/{name}/exits?minutes={minutes}&limit={limit}"),
            )
            .await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                let data: ExitsResponse = serde_json::from_value(body)?;
                print_exits(name, &data.events, &data.stats);
            }
        }

        Commands::Net { minutes, json } => {
            let body = fetch_json(&api, &format!("/api/v1/system?minutes={minutes}")).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                let data: SystemResponse = serde_json::from_value(body)?;
                print_net_table(&data.points, data.latest.as_ref());
            }
        }

        Commands::Reload => {
            let resp = api.post("/api/v1/reload").send().await?;
            print_api_response(resp).await?;
        }

        Commands::Run
        | Commands::Install { .. }
        | Commands::Uninstall { .. }
        | Commands::Check
        | Commands::Daemon { .. } => {
            unreachable!()
        }
    }

    Ok(())
}

fn print_status_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let show_ports = processes.iter().any(|p| !p.listen_ports.is_empty());

    let mut header = vec!["名称", "状态", "PID", "运行时间", "重启次数"];
    if show_ports {
        header.push("端口");
    }
    header.push("连接");
    let mut table = new_table(header);

    for p in processes {
        let status_cell = match &p.status {
            gugu_core::process::ProcessStatus::Running => Cell::new("运行中").fg(Color::Green),
            gugu_core::process::ProcessStatus::Stopped => Cell::new("已停止").fg(Color::Yellow),
            gugu_core::process::ProcessStatus::Starting => Cell::new("启动中").fg(Color::Cyan),
            gugu_core::process::ProcessStatus::Failed(e) => {
                Cell::new(format!("失败: {e}")).fg(Color::Red)
            }
            gugu_core::process::ProcessStatus::Restarting => Cell::new("重启中").fg(Color::Magenta),
            _ => Cell::new("未知").fg(Color::White),
        };

        let uptime = p
            .uptime_secs
            .map(|s| format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60))
            .unwrap_or_default();

        let mut row = vec![
            Cell::new(&p.name),
            status_cell,
            Cell::new(p.pid.map(|id| id.to_string()).unwrap_or_default()),
            Cell::new(uptime),
            Cell::new(p.restart_count),
        ];
        if show_ports {
            row.push(Cell::new(join_ports(&p.listen_ports)));
        }
        row.push(Cell::new(
            if p.connections > 0 || !p.listen_ports.is_empty() {
                p.connections.to_string()
            } else {
                String::new()
            },
        ));
        table.add_row(row);
    }

    println!("{table}");
}

fn join_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn print_list_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("名称").add_attribute(Attribute::Bold),
        Cell::new("命令").add_attribute(Attribute::Bold),
        Cell::new("自动启动").add_attribute(Attribute::Bold),
        Cell::new("自动重启").add_attribute(Attribute::Bold),
    ]);

    for p in processes {
        let cmd = if p.args.is_empty() {
            p.command.clone()
        } else {
            format!("{} {}", p.command, p.args.join(" "))
        };
        table.add_row(vec![
            Cell::new(&p.name),
            Cell::new(cmd),
            Cell::new(if p.auto_start { "是" } else { "否" }),
            Cell::new(if p.auto_restart { "是" } else { "否" }),
        ]);
    }

    println!("{table}");
}

/// 取一个 JSON 接口的响应体；非 2xx 时把错误信息带出来。
async fn fetch_json(api: &ApiHelper<'_>, path: &str) -> Result<serde_json::Value> {
    let resp = api.get(path).send().await.context("无法连接到守护进程")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_string)))
            .unwrap_or(text);
        anyhow::bail!("请求失败 ({status}): {msg}");
    }
    Ok(serde_json::from_str(&text)?)
}

fn new_table(header: Vec<&str>) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(
        header
            .into_iter()
            .map(|h| Cell::new(h).add_attribute(Attribute::Bold)),
    );
    table
}

fn format_size(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes.abs();
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let sign = if bytes < 0.0 { "-" } else { "" };
    format!("{sign}{value:.1}{}", UNITS[unit])
}

fn format_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_size(bytes_per_sec))
}

fn print_history_table(name: &str, points: &[MetricPoint]) {
    if points.is_empty() {
        println!("进程 {name} 暂无指标记录（守护进程启动后每分钟采样一次）");
        return;
    }
    let mut table = new_table(vec![
        "时间",
        "CPU 平均",
        "CPU 峰值",
        "内存 平均",
        "内存 峰值",
        "连接数",
    ]);
    for p in points {
        table.add_row(vec![
            Cell::new(p.at.format("%m-%d %H:%M")),
            Cell::new(format!("{:.1}%", p.cpu_avg)),
            Cell::new(format!("{:.1}%", p.cpu_peak)),
            Cell::new(format_size(p.mem_avg as f64)),
            Cell::new(format_size(p.mem_peak as f64)),
            Cell::new(p.connections),
        ]);
    }
    println!("{table}");
    println!("共 {} 个采样分钟，最近 24 小时", points.len());
}

fn print_exits(name: &str, events: &[ExitEvent], stats: &ExitStats) {
    let availability = stats
        .availability
        .map(|a| format!("{:.2}%", f64::from(a) * 100.0))
        .unwrap_or_else(|| "无记录".to_string());
    println!("进程 {name} 最近 {} 分钟：", stats.window_minutes);
    println!(
        "退出 {} 次（崩溃 {} 次，主动停止 {} 次，自动拉起 {} 次），累计运行 {}，可用率 {availability}",
        stats.exits,
        stats.crashes,
        stats.manual_stops,
        stats.auto_restarted,
        format_uptime(stats.uptime_secs as i64),
    );
    if events.is_empty() {
        println!("该窗口内没有退出记录");
        return;
    }
    let mut table = new_table(vec!["时间", "退出码", "存活时长", "原因", "自动拉起"]);
    for e in events {
        table.add_row(vec![
            Cell::new(e.at.format("%m-%d %H:%M:%S")),
            Cell::new(
                e.code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
            ),
            Cell::new(format_uptime(e.uptime_secs.unwrap_or(0))),
            Cell::new(if e.stopped {
                "主动停止"
            } else {
                "自行退出"
            }),
            Cell::new(if e.auto_restarted { "是" } else { "否" }),
        ]);
    }
    println!("{table}");
}

fn print_net_table(points: &[TrafficPoint], latest: Option<&TrafficPoint>) {
    if points.is_empty() {
        let current = latest.map_or_else(
            || "当前无读数".to_string(),
            |l| {
                format!(
                    "当前 ↓{} ↑{}",
                    format_rate(l.rx_bytes_per_sec),
                    format_rate(l.tx_bytes_per_sec)
                )
            },
        );
        println!("暂无带宽记录（守护进程启动后每分钟采样一次，{current}）");
        return;
    }
    let mut table = new_table(vec!["时间", "下行", "上行"]);
    for p in points {
        table.add_row(vec![
            Cell::new(p.at.format("%m-%d %H:%M")),
            Cell::new(format_rate(p.rx_bytes_per_sec)),
            Cell::new(format_rate(p.tx_bytes_per_sec)),
        ]);
    }
    println!("{table}");
    println!("整机所有物理网卡合计（不含回环），非单进程口径");
}

fn format_uptime(secs: i64) -> String {
    let secs = secs.max(0);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// 历史接口的响应体：只声明 CLI 需要打印的字段。
#[derive(serde::Deserialize)]
struct HistoryResponse {
    #[serde(default)]
    points: Vec<MetricPoint>,
}

#[derive(serde::Deserialize)]
struct ExitsResponse {
    #[serde(default)]
    events: Vec<ExitEvent>,
    stats: ExitStats,
}

#[derive(serde::Deserialize)]
struct SystemResponse {
    #[serde(default)]
    latest: Option<TrafficPoint>,
    #[serde(default)]
    points: Vec<TrafficPoint>,
}

async fn print_api_response(resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            val.get("error")
                .map(|e| e.as_str().unwrap_or(&text).to_string())
                .unwrap_or(text.clone())
        } else {
            text
        };
        anyhow::bail!("请求失败 ({status}): {msg}");
    }
    let body: serde_json::Value = resp.json().await?;
    if let Some(msg) = body.get("message") {
        println!("{msg}");
    } else if let Some(err) = body.get("error") {
        eprintln!("错误: {err}");
    }
    Ok(())
}
