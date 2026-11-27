// 指标页：CPU / 内存 / 连接数折线、退出统计与整机带宽。
// 只在页面处于激活状态时拉取，每 30 秒一轮；切走即停表。

import { $ } from '../ui/dom.js';
import { api } from '../core/api.js';
import { esc, fmtBytes, fmtDuration, fmtRate, fmtDateTime } from '../core/format.js';
import { toast } from '../ui/toast.js';

const WINDOW_MINUTES = 60;
const REFRESH_MS = 30000;

let target = null;
let timer = null;

export function initMetrics() {}

export function metricsActivate(name, tabActive) {
    if (!tabActive) {
        target = null;
        stopTimer();
        return;
    }
    if (target === name) return;
    target = name;
    $('metrics-body').innerHTML = '<div class="chart-empty">读取中…</div>';
    load();
    stopTimer();
    timer = setTimeout(function tick() {
        if (!target) return;
        load();
        timer = setTimeout(tick, REFRESH_MS);
    }, REFRESH_MS);
}

function stopTimer() {
    clearTimeout(timer);
    timer = null;
}

async function load() {
    const name = target;
    if (!name) return;
    try {
        const [history, exits, system] = await Promise.all([
            api('GET', `/processes/${encodeURIComponent(name)}/history?minutes=${WINDOW_MINUTES}`),
            api('GET', `/processes/${encodeURIComponent(name)}/exits?minutes=${WINDOW_MINUTES}&limit=20`),
            api('GET', `/system?minutes=${WINDOW_MINUTES}`),
        ]);
        if (target !== name) return;
        render(history?.points || [], exits || {}, system || {});
    } catch (error) {
        if (error.message === 'Unauthorized') return;
        if (target !== name) return;
        $('metrics-body').innerHTML = `<div class="chart-empty">读取失败：${esc(error.message || '未知错误')}</div>`;
        toast(error.message || '读取指标失败', 'error');
    }
}

function render(points, exitData, system) {
    const body = $('metrics-body');
    const stats = exitData.stats || {};
    const last = points.length ? points[points.length - 1] : null;
    let html = '';

    if (last) {
        html += `<div class="metric-tiles">
            ${tile('CPU 均值 / 峰值', `${last.cpu_avg.toFixed(1)}% / ${last.cpu_peak.toFixed(1)}%`)}
            ${tile('内存均值 / 峰值', `${fmtBytes(last.mem_avg)} / ${fmtBytes(last.mem_peak)}`)}
            ${tile('连接数峰值', String(last.connections))}
            ${tile('最近采样', new Date(last.at).toLocaleTimeString())}
        </div>`;
        html += block('CPU 占用（% 单核）', chart([
            { label: '均值', color: 'var(--accent)', values: points.map((p) => p.cpu_avg), format: (v) => `${v.toFixed(1)}%` },
            { label: '峰值', color: 'var(--violet)', values: points.map((p) => p.cpu_peak), format: (v) => `${v.toFixed(1)}%` },
        ]));
        html += block('内存（进程树合计）', chart([
            { label: '均值', color: 'var(--ok)', values: points.map((p) => p.mem_avg), format: fmtBytes },
            { label: '峰值', color: 'var(--warn)', values: points.map((p) => p.mem_peak), format: fmtBytes },
        ]));
        html += block('已建立连接数', chart([
            { label: '连接', color: 'var(--info)', values: points.map((p) => p.connections), format: (v) => String(Math.round(v)) },
        ]), `${points.length} 个采样分钟`);
    } else {
        html += '<div class="chart-empty">暂无指标记录：守护进程每分钟采样一次，运行满一分钟后再看</div>';
    }

    const availability = stats.availability == null ? '无记录' : `${(stats.availability * 100).toFixed(2)}%`;
    html += `<div class="metric-block">
        <div class="metric-block-head"><span>退出统计</span><span class="metric-summary">最近 ${stats.window_minutes ?? WINDOW_MINUTES} 分钟</span></div>
        <div class="metric-tiles">
            ${tile('退出次数', String(stats.exits ?? 0))}
            ${tile('崩溃', String(stats.crashes ?? 0))}
            ${tile('主动停止', String(stats.manual_stops ?? 0))}
            ${tile('自动拉起', String(stats.auto_restarted ?? 0))}
            ${tile('可用率', availability)}
        </div>
        ${exitTable(exitData.events || [])}
    </div>`;

    const sysPoints = system.points || [];
    const latest = system.latest;
    html += `<div class="metric-block">
        <div class="metric-block-head">
            <span>整机带宽</span>
            <span class="metric-summary">${latest ? `当前 ↓${fmtRate(latest.rx_bytes_per_sec)} ↑${fmtRate(latest.tx_bytes_per_sec)}` : '等待采样'}</span>
        </div>
        ${chart([
            { label: '下行', color: 'var(--info)', values: sysPoints.map((p) => p.rx_bytes_per_sec), format: fmtRate },
            { label: '上行', color: 'var(--warn)', values: sysPoints.map((p) => p.tx_bytes_per_sec), format: fmtRate },
        ])}
        <p class="metric-note">所有物理网卡合计（不含回环），不是单进程口径</p>
    </div>`;

    body.innerHTML = html;
}

function tile(label, value) {
    return `<div class="metric-tile"><span class="metric-tile-label">${esc(label)}</span><span class="metric-tile-value">${esc(value)}</span></div>`;
}

function block(title, content, summary = '') {
    return `<div class="metric-block">
        <div class="metric-block-head"><span>${esc(title)}</span>${summary ? `<span class="metric-summary">${esc(summary)}</span>` : ''}</div>
        ${content}
    </div>`;
}

/// 折线按最大值归一化，同一张图里的多条线共享纵轴。
function chart(series) {
    const values = series.flatMap((s) => s.values);
    if (!values.length) return '<div class="chart-empty">暂无采样</div>';

    const W = 560;
    const H = 108;
    const PAD = 10;
    const max = Math.max(...values, 1e-9);
    const count = Math.max(...series.map((s) => s.values.length));
    const innerW = W - PAD * 2;
    const innerH = H - PAD * 2;
    const stepX = count > 1 ? innerW / (count - 1) : 0;

    const paths = series.map((s) => {
        const d = s.values.map((value, index) => {
            const x = PAD + index * stepX;
            const y = PAD + innerH - (value / max) * innerH;
            return `${index ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
        }).join(' ');
        return `<path d="${d}" fill="none" stroke="${s.color}" stroke-width="1.5" vector-effect="non-scaling-stroke" stroke-linejoin="round"/>`;
    }).join('');

    const legend = series.map((s) => `<span><i style="background:${s.color}"></i>${esc(s.label)}</span>`).join('');
    const format = series[0].format || ((value) => String(value));

    return `<div class="chart-wrap">
        <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img" aria-label="折线图">${paths}</svg>
        <div class="chart-scale"><span>0</span><span>${esc(format(max))}</span></div>
        <div class="chart-legend">${legend}</div>
    </div>`;
}

function exitTable(events) {
    if (!events.length) return '<div class="chart-empty">该窗口内没有退出记录</div>';
    const rows = events.map((event) => {
        const code = event.code == null ? '未知' : event.code;
        const tone = event.stopped ? 'muted' : (event.code === 0 ? 'ok' : 'bad');
        return `<tr>
            <td>${esc(fmtDateTime(event.at))}</td>
            <td class="${tone}">${esc(String(code))}</td>
            <td>${event.uptime_secs == null ? '—' : esc(fmtDuration(event.uptime_secs))}</td>
            <td>${event.stopped ? '主动停止' : '自行退出'}</td>
            <td>${event.auto_restarted ? '是' : '否'}</td>
        </tr>`;
    }).join('');
    return `<table class="history-table">
        <thead><tr><th>时间</th><th>退出码</th><th>存活时长</th><th>原因</th><th>自动拉起</th></tr></thead>
        <tbody>${rows}</tbody>
    </table>`;
}
