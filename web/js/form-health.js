// 健康检查与网络设置表单：字段收集、回填、重置，及类型切换显隐。

function toggleHcFields() {
    const type = document.getElementById('f-hc-type').value;
    document.getElementById('hc-tcp-fields').style.display = type === 'tcp' ? '' : 'none';
    document.getElementById('hc-http-fields').style.display = type === 'http' ? '' : 'none';
    document.getElementById('hc-common-fields').style.display = type ? '' : 'none';
}

function collectHealthCheck() {
    const type = document.getElementById('f-hc-type').value;
    if (!type) return { health_check: null, unhealthy_restart: false };
    const hc = {
        type,
        interval_secs: parseInt(document.getElementById('f-hc-interval').value) || 30,
        timeout_secs: parseInt(document.getElementById('f-hc-timeout').value) || 5,
        fail_threshold: parseInt(document.getElementById('f-hc-fail').value) || 3,
    };
    if (type === 'tcp') {
        const port = parseInt(document.getElementById('f-hc-port').value);
        if (!port) return null;
        const host = document.getElementById('f-hc-host').value.trim();
        if (host) hc.host = host;
        hc.port = port;
    } else {
        const url = document.getElementById('f-hc-url').value.trim();
        if (!url) return null;
        hc.url = url;
    }
    return {
        health_check: hc,
        unhealthy_restart: document.getElementById('f-unhealthy-restart').checked,
    };
}

function fillHealthCheck(cfg) {
    const hc = cfg.health_check;
    if (!hc) {
        document.getElementById('f-hc-type').value = '';
        toggleHcFields();
        return;
    }
    const type = hc.type || '';
    document.getElementById('f-hc-type').value = type;
    if (type === 'tcp') {
        document.getElementById('f-hc-host').value = hc.host || '';
        document.getElementById('f-hc-port').value = hc.port || '';
    }
    if (type === 'http') document.getElementById('f-hc-url').value = hc.url || '';
    document.getElementById('f-hc-interval').value = hc.interval_secs || 30;
    document.getElementById('f-hc-timeout').value = hc.timeout_secs || 5;
    document.getElementById('f-hc-fail').value = hc.fail_threshold || 3;
    document.getElementById('f-unhealthy-restart').checked = cfg.unhealthy_restart || false;
    toggleHcFields();
}

function resetHealthCheck() {
    document.getElementById('f-hc-type').value = '';
    document.getElementById('f-hc-host').value = '';
    document.getElementById('f-hc-port').value = '';
    document.getElementById('f-hc-url').value = '';
    document.getElementById('f-hc-interval').value = 30;
    document.getElementById('f-hc-timeout').value = 5;
    document.getElementById('f-hc-fail').value = 3;
    document.getElementById('f-unhealthy-restart').checked = false;
    toggleHcFields();
}

function collectPorts() {
    const raw = document.getElementById('f-ports').value;
    return raw
        .split(/[,，\s]+/)
        .map(s => parseInt(s, 10))
        .filter(n => Number.isInteger(n) && n > 0 && n <= 65535);
}

/// 端口/就绪/日志告警三块拼成一个对象，避免 onsubmit 里散落大量读取。
function collectNetworkSettings() {
    const patterns = document.getElementById('f-alert-patterns').value
        .split(/[,，]+/)
        .map(s => s.trim())
        .filter(Boolean);
    const burst = parseInt(document.getElementById('f-alert-burst').value) || 0;
    const log_alert = (patterns.length || burst > 0) ? {
        patterns,
        window_secs: parseInt(document.getElementById('f-alert-window').value) || 10,
        burst_lines: burst,
        cooldown_secs: parseInt(document.getElementById('f-alert-cooldown').value) || 300,
    } : null;
    return {
        listen_ports: collectPorts(),
        port_check: document.getElementById('f-port-check').checked,
        ready_timeout_secs: parseInt(document.getElementById('f-ready-timeout').value) || 60,
        log_alert,
    };
}

function fillNetworkSettings(cfg) {
    document.getElementById('f-ports').value = (cfg.listen_ports || []).join(', ');
    document.getElementById('f-port-check').checked = cfg.port_check ?? true;
    document.getElementById('f-ready-timeout').value = cfg.ready_timeout_secs ?? 60;
    const alert = cfg.log_alert;
    document.getElementById('f-alert-patterns').value = alert ? (alert.patterns || []).join(', ') : '';
    document.getElementById('f-alert-burst').value = alert ? (alert.burst_lines ?? 0) : 0;
    document.getElementById('f-alert-window').value = alert ? (alert.window_secs ?? 10) : 10;
    document.getElementById('f-alert-cooldown').value = alert ? (alert.cooldown_secs ?? 300) : 300;
}

function resetNetworkSettings() {
    document.getElementById('f-ports').value = '';
    document.getElementById('f-port-check').checked = true;
    document.getElementById('f-ready-timeout').value = 60;
    document.getElementById('f-alert-patterns').value = '';
    document.getElementById('f-alert-burst').value = 0;
    document.getElementById('f-alert-window').value = 10;
    document.getElementById('f-alert-cooldown').value = 300;
}

document.getElementById('f-hc-type').addEventListener('change', toggleHcFields);
