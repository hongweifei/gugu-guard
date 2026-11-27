//! CLI 输出：表格、日志行与各类格式化。
//!
//! 只处理“已经拿到手的值”，不发起请求；响应体形状也在这里声明，
//! 字段名拼错会在反序列化时报错，而不是静默变成 0。

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Attribute, Cell, Color, Table,
};
use gugu_core::audit::AuditEntry;
use gugu_core::history::{ExitEvent, ExitStats, MetricPoint, TrafficPoint};
use gugu_core::logs::{LogLevel, LogSearchReport, ParsedLine};
use gugu_core::process::{LogEntry, LogStream};
use serde::{Deserialize, Serialize};

pub(crate) fn print_status_table(processes: &[gugu_core::process::ProcessInfo]) {
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

pub(crate) fn print_list_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let mut table = new_table(vec!["名称", "命令", "自动启动", "自动重启"]);
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

/// `GET /api/v1/nodes` 的响应体。
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct NodesResponse {
    #[serde(default)]
    pub local_name: String,
    #[serde(default)]
    pub nodes: Vec<NodeRow>,
}

/// 节点表里的一行，字段与 `NodeView` 对齐。
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct NodeRow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub processes: usize,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// `gugu nodes`：本机与从节点的在线状态、延迟与负载概览。
pub(crate) fn print_nodes_table(body: &NodesResponse) {
    if body.nodes.is_empty() {
        println!("节点表为空");
        return;
    }
    let mut table = new_table(vec![
        "节点",
        "显示名",
        "地址",
        "状态",
        "延迟",
        "版本",
        "进程",
        "备注",
    ]);
    for node in &body.nodes {
        let state = if node.online {
            Cell::new("在线").fg(Color::Green)
        } else {
            Cell::new("离线").fg(Color::Red)
        };
        let latency = node
            .latency_ms
            .map_or_else(|| "-".to_string(), |ms| format!("{ms} ms"));
        table.add_row(vec![
            Cell::new(&node.id),
            Cell::new(&node.name),
            Cell::new(node.url.as_deref().unwrap_or("本机")),
            state,
            Cell::new(latency),
            Cell::new(node.version.as_deref().unwrap_or("-")),
            Cell::new(format!("{}/{}", node.running, node.processes)),
            Cell::new(node.last_error.as_deref().unwrap_or("")),
        ]);
    }

    println!("{table}");
    println!(
        "共 {} 个节点；用 --node <节点> 操作从节点",
        body.nodes.len()
    );
}

pub(crate) fn print_log_entries(entries: &[LogEntry]) {
    for entry in entries {
        print_log_line(
            Some(entry.timestamp.with_timezone(&chrono::Local)),
            &entry.stream,
            entry.parsed.as_ref().and_then(|p| p.level),
            log_text(&entry.line, entry.parsed.as_ref()),
        );
    }
    if entries.is_empty() {
        println!("暂无日志");
    }
}

pub(crate) fn print_log_report(report: &LogSearchReport) {
    for hit in &report.hits {
        let at = hit.at().map(|at| at.with_timezone(&chrono::Local));
        print_log_line(
            at,
            &hit.stream,
            hit.parsed.as_ref().and_then(|p| p.level),
            log_text(&hit.line, hit.parsed.as_ref()),
        );
    }
    if report.hits.is_empty() {
        println!("没有命中的日志");
    }
    let files = if report.files.is_empty() {
        String::new()
    } else {
        format!("；文件 {}", report.files.join(", "))
    };
    println!(
        "扫描 {} 行 / {}，命中 {} 条{}{files}",
        report.scanned_lines,
        format_size(report.scanned_bytes as f64),
        report.matched,
        if report.truncated {
            "，结果已截断（只保留最近的命中）"
        } else {
            ""
        }
    );
}

/// 有结构化正文时用正文，否则原样打印整行。
fn log_text<'a>(line: &'a str, parsed: Option<&'a ParsedLine>) -> &'a str {
    parsed
        .and_then(|parsed| parsed.message.as_deref())
        .unwrap_or(line)
}

fn print_log_line(
    at: Option<chrono::DateTime<chrono::Local>>,
    stream: &LogStream,
    level: Option<LogLevel>,
    text: &str,
) {
    let time = at.map_or_else(
        || "--:--:--".to_string(),
        |at| at.format("%H:%M:%S").to_string(),
    );
    let stream_tag = match stream {
        LogStream::Stdout => "OUT",
        LogStream::Stderr => "ERR",
        _ => "???",
    };
    let level_tag = level.map_or_else(
        || "     ".to_string(),
        |level| format!("{:<5}", level.as_str().to_uppercase()),
    );
    println!("[{time}] [{stream_tag}] {level_tag} {text}");
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

/// 历史接口的响应体：只声明 CLI 需要打印的字段。
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct HistoryResponse {
    #[serde(default)]
    pub points: Vec<MetricPoint>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ExitsResponse {
    #[serde(default)]
    pub events: Vec<ExitEvent>,
    pub stats: ExitStats,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct SystemResponse {
    #[serde(default)]
    pub latest: Option<TrafficPoint>,
    #[serde(default)]
    pub points: Vec<TrafficPoint>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AuditResponse {
    #[serde(default)]
    pub entries: Vec<AuditEntry>,
    #[serde(default)]
    pub retention_days: i64,
}

pub(crate) fn print_history_table(name: &str, points: &[MetricPoint]) {
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

pub(crate) fn print_exits(name: &str, events: &[ExitEvent], stats: &ExitStats) {
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

pub(crate) fn print_net_table(points: &[TrafficPoint], latest: Option<&TrafficPoint>) {
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

pub(crate) fn print_audit(entries: &[AuditEntry], retention_days: i64) {
    if entries.is_empty() {
        println!("暂无操作记录（只记录会改变状态的写操作）");
        return;
    }
    let show_detail = entries.iter().any(|entry| entry.detail.is_some());
    let mut header = vec!["时间", "操作", "对象", "结果", "来源", "客户端", "耗时"];
    if show_detail {
        header.push("备注");
    }
    let mut table = new_table(header);
    for entry in entries {
        let at = entry.at.with_timezone(&chrono::Local);
        let mut row = vec![
            Cell::new(at.format("%m-%d %H:%M:%S")),
            Cell::new(format!("{} ({})", entry.label, entry.action)),
            Cell::new(entry.target.as_deref().unwrap_or("-")),
            if entry.ok {
                Cell::new("成功").fg(Color::Green)
            } else {
                Cell::new(format!("失败 {}", entry.status)).fg(Color::Red)
            },
            Cell::new(&entry.client),
            Cell::new(actor_of(entry)),
            Cell::new(format_duration(entry.duration_ms)),
        ];
        if show_detail {
            row.push(Cell::new(entry.detail.as_deref().unwrap_or("")));
        }
        table.add_row(row);
    }
    println!("{table}");
    println!(
        "共 {} 条（新的在前），保留最近 {retention_days} 天",
        entries.len()
    );
}

/// 客户端类型：CLI 自带 UA，浏览器带 Mozilla，其余原样展示。
fn actor_of(entry: &AuditEntry) -> String {
    let Some(user_agent) = entry.user_agent.as_deref() else {
        return "-".to_string();
    };
    if user_agent.starts_with("gugu-cli/") {
        return format!("CLI ({})", user_agent.trim_start_matches("gugu-cli/"));
    }
    if user_agent.contains("PowerShell") {
        return "PowerShell".to_string();
    }
    if user_agent.contains("Mozilla") {
        return "WebUI".to_string();
    }
    user_agent.chars().take(24).collect()
}

fn format_duration(millis: u64) -> String {
    if millis == 0 {
        "<1 ms".to_string()
    } else if millis < 1000 {
        format!("{millis} ms")
    } else {
        format!("{:.1} s", millis as f64 / 1000.0)
    }
}
