//! CLI 客户端子命令：连接守护进程 HTTP API 并输出表格/文本。

use anyhow::{Context, Result};
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, Color, Table};
use gugu_core::config::{AppConfig, DEFAULT_BIND_ADDR, DEFAULT_PORT, LOCALHOST};
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
    let connect_addr = if addr == DEFAULT_BIND_ADDR { LOCALHOST } else { addr };
    Ok(format!("http://{connect_addr}:{port}"))
}

pub(crate) fn get_api_key(config_path: &Path, explicit: Option<&str>) -> Result<Option<String>> {
    if let Some(key) = explicit {
        return Ok(Some(key.to_string()));
    }
    let config = AppConfig::load(config_path).ok();
    Ok(config.and_then(|c| c.daemon.api_key))
}

pub(crate) async fn run_client(command: &Commands, server_url: &str, api_key: Option<&str>) -> Result<()> {
    let client = reqwest::Client::new();
    let api = ApiHelper { client: &client, base: server_url, key: api_key };

    match command {
        Commands::Status { json } => {
            let resp = api.get("/api/v1/processes")
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
            let resp = api.get("/api/v1/processes")
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
            let resp = api.post(&format!("/api/v1/processes/{name}/start")).send().await?;
            print_api_response(resp).await?;
        }

        Commands::Stop { name } => {
            let resp = api.post(&format!("/api/v1/processes/{name}/stop")).send().await?;
            print_api_response(resp).await?;
        }

        Commands::Restart { name } => {
            let resp = api.post(&format!("/api/v1/processes/{name}/restart")).send().await?;
            print_api_response(resp).await?;
        }

        Commands::Add {
            name, command, args, dir, env,
            no_auto_start, no_auto_restart, max_restarts,
            restart_delay, stop_timeout, stdout_log, stderr_log, start,
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
                "start_now": start,
            });
            let resp = api.post(&format!("/api/v1/processes/{name}"))
                .json(&body)
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Remove { name } => {
            let resp = api.delete(&format!("/api/v1/processes/{name}")).send().await?;
            print_api_response(resp).await?;
        }

        Commands::Logs { name, lines } => {
            let resp = api.get(&format!("/api/v1/processes/{name}/logs?lines={lines}")).send().await?;
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

        Commands::Reload => {
            let resp = api.post("/api/v1/reload").send().await?;
            print_api_response(resp).await?;
        }

        Commands::Run | Commands::Install { .. } | Commands::Uninstall { .. } | Commands::Check => {
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

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("名称").add_attribute(Attribute::Bold),
        Cell::new("状态").add_attribute(Attribute::Bold),
        Cell::new("PID").add_attribute(Attribute::Bold),
        Cell::new("运行时间").add_attribute(Attribute::Bold),
        Cell::new("重启次数").add_attribute(Attribute::Bold),
    ]);

    for p in processes {
        let status_cell = match &p.status {
            gugu_core::process::ProcessStatus::Running => Cell::new("运行中").fg(Color::Green),
            gugu_core::process::ProcessStatus::Stopped => Cell::new("已停止").fg(Color::Yellow),
            gugu_core::process::ProcessStatus::Starting => Cell::new("启动中").fg(Color::Cyan),
            gugu_core::process::ProcessStatus::Failed(e) => Cell::new(format!("失败: {e}")).fg(Color::Red),
            gugu_core::process::ProcessStatus::Restarting => Cell::new("重启中").fg(Color::Magenta),
            _ => Cell::new("未知").fg(Color::White),
        };

        let uptime = p.uptime_secs.map(|s| {
            format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
        }).unwrap_or_default();

        table.add_row(vec![
            Cell::new(&p.name),
            status_cell,
            Cell::new(p.pid.map(|id| id.to_string()).unwrap_or_default()),
            Cell::new(uptime),
            Cell::new(p.restart_count),
        ]);
    }

    println!("{table}");
}

fn print_list_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
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
