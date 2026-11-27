//! Prometheus 文本格式的指标族渲染：每个族一个函数，按固定顺序拼装。

use gugu_core::history::TrafficPoint;
use gugu_core::process::{ProcessInfo, ProcessStatus};

/// 输出按指标族名字典序，且与旧实现一致：无样本的族不输出。
pub(super) fn exposition(processes: &[ProcessInfo], net: Option<&TrafficPoint>) -> String {
    let mut sorted: Vec<&ProcessInfo> = processes.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::with_capacity(1024);
    process_cpu(&mut out, &sorted);
    process_health(&mut out, &sorted);
    process_memory(&mut out, &sorted);
    process_connections(&mut out, &sorted);
    process_runtime(&mut out, &sorted);
    aggregates(&mut out, &sorted);
    net_traffic(&mut out, net);
    out
}

fn status_value(status: &ProcessStatus) -> i64 {
    match status {
        ProcessStatus::Running => 1,
        ProcessStatus::Starting => 2,
        ProcessStatus::Restarting => 3,
        ProcessStatus::Failed(_) => 4,
        ProcessStatus::Stopped | _ => 0,
    }
}

/// Prometheus 标签值转义：反斜杠、双引号、换行。
fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn header(out: &mut String, name: &str, help: &str) {
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
}

fn process_cpu(out: &mut String, sorted: &[&ProcessInfo]) {
    if !sorted.iter().any(|p| p.usage.is_some()) {
        return;
    }
    header(
        out,
        "gugu_process_cpu_percent",
        "Process tree CPU usage as a percentage of one core",
    );
    for p in sorted {
        if let Some(usage) = p.usage {
            out.push_str(&format!(
                "gugu_process_cpu_percent{{process=\"{}\"}} {:.2}\n",
                escape_label(&p.name),
                usage.cpu_percent
            ));
        }
    }
}

fn process_health(out: &mut String, sorted: &[&ProcessInfo]) {
    if !sorted.iter().any(|p| p.healthy.is_some()) {
        return;
    }
    header(
        out,
        "gugu_process_health_status",
        "Health check result: 1=healthy, 0=unhealthy",
    );
    for p in sorted {
        if let Some(healthy) = p.healthy {
            out.push_str(&format!(
                "gugu_process_health_status{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                i64::from(healthy)
            ));
        }
    }
}

fn process_memory(out: &mut String, sorted: &[&ProcessInfo]) {
    if !sorted.iter().any(|p| p.usage.is_some()) {
        return;
    }
    header(
        out,
        "gugu_process_memory_bytes",
        "Process tree resident memory in bytes",
    );
    for p in sorted {
        if let Some(usage) = p.usage {
            out.push_str(&format!(
                "gugu_process_memory_bytes{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                usage.memory_bytes
            ));
        }
    }
}

fn process_connections(out: &mut String, sorted: &[&ProcessInfo]) {
    if !sorted.iter().any(|p| !p.listen_ports.is_empty()) {
        return;
    }
    header(
        out,
        "gugu_process_connections",
        "Established TCP connections on the process watched ports",
    );
    for p in sorted {
        if !p.listen_ports.is_empty() {
            out.push_str(&format!(
                "gugu_process_connections{{process=\"{}\"}} {}\n",
                escape_label(&p.name),
                p.connections
            ));
        }
    }
}

/// 有进程时才输出的三族：重启次数、状态、运行时长。
fn process_runtime(out: &mut String, sorted: &[&ProcessInfo]) {
    if sorted.is_empty() {
        return;
    }
    header(
        out,
        "gugu_process_restarts_total",
        "Number of crash restarts for the process",
    );
    for p in sorted {
        out.push_str(&format!(
            "gugu_process_restarts_total{{process=\"{}\"}} {}\n",
            escape_label(&p.name),
            p.restart_count
        ));
    }

    header(
        out,
        "gugu_process_status",
        "Process status: 1=running, 0=stopped, 2=starting, 3=restarting, 4=failed",
    );
    for p in sorted {
        out.push_str(&format!(
            "gugu_process_status{{process=\"{}\"}} {}\n",
            escape_label(&p.name),
            status_value(&p.status)
        ));
    }

    header(
        out,
        "gugu_process_uptime_seconds",
        "Process uptime in seconds",
    );
    for p in sorted {
        out.push_str(&format!(
            "gugu_process_uptime_seconds{{process=\"{}\"}} {}\n",
            escape_label(&p.name),
            p.uptime_secs.unwrap_or(0)
        ));
    }
}

fn aggregates(out: &mut String, sorted: &[&ProcessInfo]) {
    let mut running = 0i64;
    let mut stopped = 0i64;
    let mut failed = 0i64;
    for p in sorted {
        match &p.status {
            ProcessStatus::Running => running += 1,
            ProcessStatus::Stopped => stopped += 1,
            ProcessStatus::Failed(_) => failed += 1,
            _ => {}
        }
    }

    header(out, "gugu_processes", "Total number of managed processes");
    out.push_str(&format!("gugu_processes {}\n", sorted.len()));

    header(out, "gugu_processes_failed", "Number of failed processes");
    out.push_str(&format!("gugu_processes_failed {failed}\n"));

    header(out, "gugu_processes_running", "Number of running processes");
    out.push_str(&format!("gugu_processes_running {running}\n"));

    header(out, "gugu_processes_stopped", "Number of stopped processes");
    out.push_str(&format!("gugu_processes_stopped {stopped}\n"));
}

fn net_traffic(out: &mut String, net: Option<&TrafficPoint>) {
    let Some(net) = net else {
        return;
    };
    header(
        out,
        "gugu_system_net_rx_bytes_per_sec",
        "Host-wide network receive rate in bytes per second",
    );
    out.push_str(&format!(
        "gugu_system_net_rx_bytes_per_sec {:.2}\n",
        net.rx_bytes_per_sec
    ));

    header(
        out,
        "gugu_system_net_tx_bytes_per_sec",
        "Host-wide network transmit rate in bytes per second",
    );
    out.push_str(&format!(
        "gugu_system_net_tx_bytes_per_sec {:.2}\n",
        net.tx_bytes_per_sec
    ));
}
