// 指标面板：进程 CPU / 内存 / 连接数折线、退出历史、整机带宽。
// 数据来自三个只读接口 (/processes/{name}/history、/processes/{name}/exits、/system)，
// 面板打开期间每 30 秒刷新一次，关闭即停表。

const METRIC_WINDOW_MINUTES = 60;
const METRIC_REFRESH_MS = 30000;

let metricProcess = null;
let metricTimer = null;

function sparkSvg(series) {
    const values = series.flatMap(s => s.values);
    if (!values.length) return '<div class="chart-empty">暂无采样（守护进程每分钟记录一次）</div>';
    const W = 560, H = 110, PAD = 10;
    const max = Math.max(...values, 1e-9);
    const count = Math.max(...series.map(s => s.values.length));
    const innerW = W - PAD * 2;
    const innerH = H - PAD * 2;
    const stepX = count > 1 ? innerW / (count - 1) : 0;
    const paths = series.map(s => {
        const d = s.values.map((v, i) => {
            const x = PAD + i * stepX;
            const y = PAD + innerH - (v / max) * innerH;
            return `${i ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
        }).join(' ');
        return `<path d="${d}" fill="none" stroke="${s.color}" stroke-width="1.5" vector-effect="non-scaling-stroke" stroke-linejoin="round"/>`;
    }).join('');
    const legend = series.map(s => `<span class="chart-legend"><i style="background:${s.color}"></i>${s.label}</span>`).join('');
    const fmt = series[0].format || ((v) => String(v));
    return `<div class="chart-wrap">
        <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">${paths}</svg>
        <div class="chart-scale"><span>0</span><span>${esc(fmt(max))}</span></div>
        <div class="chart-legend-row">${legend}</div>
    </div>`;
}

function metricBlock(title, series, summary) {
    return `<div class="metric-block">
        <div class="metric-block-head"><span>${esc(title)}</span>${summary ? `<span class="metric-summary">${summary}</span>` : ''}</div>
        ${sparkSvg(series)}
    </div>`;
}

function exitTable(events) {
    if (!events.length) return '<div class="chart-empty">该窗口内没有退出记录</div>';
    const rows = events.map(e => {
        const code = e.code == null ? '未知' : e.code;
        const cls = e.stopped ? 'muted' : (e.code === 0 ? 'ok' : 'bad');
        const up = e.uptime_secs == null ? '—' : fmtTime(e.uptime_secs);
        const reason = e.stopped ? '主动停止' : '自行退出';
        return `<tr><td>${new Date(e.at).toLocaleString()}</td><td class="${cls}">${esc(String(code))}</td><td>${up}</td><td>${reason}</td><td>${e.auto_restarted ? '是' : '否'}</td></tr>`;
    }).join('');
    return `<table class="history-table"><thead><tr><th>时间</th><th>退出码</th><th>存活时长</th><th>原因</th><th>自动拉起</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function renderMetrics(name, points, exitData, sysData) {
    const body = document.getElementById('metrics-body');
    if (!body) return;
    const stats = exitData.stats || {};
    const last = points.length ? points[points.length - 1] : null;
    const hr = (t) => new Date(t).toLocaleTimeString();

    let html = '';
    if (last) {
        html += `<div class="metric-tiles">
            <div class="metric-tile"><span class="metric-tile-label">CPU（均值 / 峰值）</span><span class="metric-tile-val">${last.cpu_avg.toFixed(1)}% / ${last.cpu_peak.toFixed(1)}%</span></div>
            <div class="metric-tile"><span class="metric-tile-label">内存（均值 / 峰值）</span><span class="metric-tile-val">${fmtBytes(last.mem_avg)} / ${fmtBytes(last.mem_peak)}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">连接数（峰值）</span><span class="metric-tile-val">${last.connections}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">最近采样</span><span class="metric-tile-val">${hr(last.at)}</span></div>
        </div>`;
        html += metricBlock('CPU 占用 (% 单核)',
            [
                { label: '均值', color: 'var(--accent)', values: points.map(p => p.cpu_avg), format: v => v.toFixed(1) + '%' },
                { label: '峰值', color: 'var(--purple)', values: points.map(p => p.cpu_peak), format: v => v.toFixed(1) + '%' },
            ]);
        html += metricBlock('内存（进程树合计）',
            [
                { label: '均值', color: 'var(--green)', values: points.map(p => p.mem_avg), format: fmtBytes },
                { label: '峰值', color: 'var(--amber)', values: points.map(p => p.mem_peak), format: fmtBytes },
            ]);
        html += metricBlock('已建立连接数',
            [{ label: '连接', color: 'var(--sky)', values: points.map(p => p.connections), format: v => String(Math.round(v)) }],
            `${points.length} 个采样分钟`);
    } else {
        html += `<div class="chart-empty">进程 ${esc(name)} 暂无指标记录：守护进程每分钟采样一次，运行一分钟后再查看</div>`;
    }

    const availability = stats.availability == null ? '无记录' : (stats.availability * 100).toFixed(2) + '%';
    html += `<div class="metric-block">
        <div class="metric-block-head"><span>退出历史</span><span class="metric-summary">最近 ${stats.window_minutes ?? METRIC_WINDOW_MINUTES} 分钟</span></div>
        <div class="metric-tiles">
            <div class="metric-tile"><span class="metric-tile-label">退出次数</span><span class="metric-tile-val">${stats.exits ?? 0}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">崩溃</span><span class="metric-tile-val">${stats.crashes ?? 0}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">主动停止</span><span class="metric-tile-val">${stats.manual_stops ?? 0}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">自动拉起</span><span class="metric-tile-val">${stats.auto_restarted ?? 0}</span></div>
            <div class="metric-tile"><span class="metric-tile-label">可用率</span><span class="metric-tile-val">${availability}</span></div>
        </div>
        ${exitTable(exitData.events || [])}
    </div>`;

    const sysPoints = sysData.points || [];
    const sysLatest = sysData.latest;
    html += `<div class="metric-block">
        <div class="metric-block-head"><span>整机带宽</span><span class="metric-summary">${sysLatest ? `当前 ↓${fmtRate(sysLatest.rx_bytes_per_sec)} ↑${fmtRate(sysLatest.tx_bytes_per_sec)}` : '等待采样'}</span></div>
        ${sparkSvg([
            { label: '下行', color: 'var(--sky)', values: sysPoints.map(p => p.rx_bytes_per_sec), format: fmtRate },
            { label: '上行', color: 'var(--amber)', values: sysPoints.map(p => p.tx_bytes_per_sec), format: fmtRate },
        ])}
        <div class="metric-note">整机所有物理网卡合计（不含回环），不是单进程口径</div>
    </div>`;

    body.innerHTML = html;
}

async function loadMetrics() {
    const name = metricProcess;
    if (!name) return;
    try {
        const [hist, exits, sys] = await Promise.all([
            api('GET', `/processes/${encodeURIComponent(name)}/history?minutes=${METRIC_WINDOW_MINUTES}`),
            api('GET', `/processes/${encodeURIComponent(name)}/exits?minutes=${METRIC_WINDOW_MINUTES}&limit=20`),
            api('GET', `/system?minutes=${METRIC_WINDOW_MINUTES}`),
        ]);
        if (metricProcess !== name) return;
        renderMetrics(name, hist.points || [], exits || {}, sys || {});
    } catch (e) {
        if (e.message === 'Unauthorized') return;
        const body = document.getElementById('metrics-body');
        if (body) body.innerHTML = `<div class="chart-empty">读取失败：${esc(e.message || '未知错误')}</div>`;
    }
}

function showMetrics(name) {
    metricProcess = name;
    const overlay = document.getElementById('metric-overlay');
    if (!overlay) return;
    document.getElementById('metric-name').textContent = name;
    document.getElementById('metrics-body').innerHTML = '<div class="chart-empty">读取中…</div>';
    overlay.classList.add('open');
    loadMetrics();
    clearTimeout(metricTimer);
    metricTimer = setTimeout(function tick() {
        if (!metricProcess) return;
        loadMetrics();
        metricTimer = setTimeout(tick, METRIC_REFRESH_MS);
    }, METRIC_REFRESH_MS);
}

function closeMetrics() {
    metricProcess = null;
    clearTimeout(metricTimer);
    metricTimer = null;
    const overlay = document.getElementById('metric-overlay');
    if (overlay) overlay.classList.remove('open');
}
