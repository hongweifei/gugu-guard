// 抽屉概览页：把当前进程的关键读数、命令、健康检查与最近退出放在一屏内。
// 数据来自进程列表本身（WS 每 2 秒推送），不额外请求。

import { $ } from '../ui/dom.js';
import { commandLine, esc, fmtBytes, fmtDateTime, fmtDuration, statusMeta } from '../core/format.js';
import { currentProcess, subscribe } from '../core/state.js';

let hooks = { onEdit: () => {}, onRemove: () => {}, onHealth: () => {} };
let open = false;

export function initOverview(config) {
    hooks = { ...hooks, ...config };
    subscribe((reason) => {
        if (!open) return;
        if (reason === 'processes' || reason === 'selection' || reason === 'reset') render();
    });
}

export function overviewActivate(isOpen) {
    open = isOpen;
    if (isOpen) render();
}

function render() {
    const body = $('panel-overview');
    const process = currentProcess();
    if (!process) {
        body.innerHTML = '<div class="ov-empty">进程已不在列表中</div>';
        return;
    }

    const status = statusMeta(process);
    const usage = process.usage;
    const ports = process.listen_ports || [];
    const exits = (process.recent_exits || []).slice(-5).reverse();

    body.innerHTML = `
        <div class="ov-head">
            <span class="td-mono td-muted">${process.pid ? `PID ${process.pid}` : '未运行'}${status.detail ? ` · ${esc(status.detail)}` : ''}</span>
        </div>

        <div class="ov-kpis">
            ${kpi('运行时长', fmtDuration(process.uptime_secs))}
            ${kpi('内存 · CPU', usage ? `${fmtBytes(usage.memory_bytes)} · ${usage.cpu_percent.toFixed(1)}%` : '—')}
            ${kpi('进程树', usage ? `${usage.process_count} 个进程` : '—')}
            ${kpi('监听端口', ports.length ? ports.join(' · ') : '未配置')}
            ${kpi('已建立连接', ports.length ? String(process.connections || 0) : '—')}
            ${kpi('自动启动', process.auto_start ? '是' : '否')}
        </div>

        <div class="ov-section">
            <div class="ov-section-title"><span>执行命令</span></div>
            <div class="ov-command">${esc(commandLine(process))}</div>
        </div>

        <div class="ov-section">
            <div class="ov-section-title">
                <span>健康检查</span>
                ${process.has_health_check
                    ? `<button type="button" class="btn btn-sm" data-act="health">立即检查</button>`
                    : ''}
            </div>
            ${process.has_health_check
                ? `<div class="ov-kpis">
                        ${kpi('当前判定', process.healthy === true ? '健康' : (process.healthy === false ? '异常' : '待检'))}
                        ${kpi('失败自动重启', process.unhealthy_restart ? '开启' : '关闭')}
                        ${kpi('重启计数', String(process.restart_count ?? 0))}
                    </div>`
                : '<div class="ov-empty">未启用健康检查</div>'}
        </div>

        <div class="ov-section">
            <div class="ov-section-title"><span>最近退出</span><span class="metric-summary">${exits.length ? `最近 ${exits.length} 次` : ''}</span></div>
            ${exits.length ? exitTable(exits) : '<div class="ov-empty">暂无退出记录</div>'}
        </div>

        <div class="ov-actions">
            <button type="button" class="btn btn-sm" data-act="edit">编辑配置</button>
            <button type="button" class="btn btn-sm" data-act="metrics">查看指标</button>
            <button type="button" class="btn btn-sm btn-danger" data-act="remove">删除进程</button>
        </div>`;

    body.querySelectorAll('[data-act]').forEach((button) => {
        button.addEventListener('click', () => {
            const name = process.name;
            switch (button.dataset.act) {
                case 'edit': hooks.onEdit(name); break;
                case 'remove': hooks.onRemove(name); break;
                case 'health': hooks.onHealth(name); break;
                case 'metrics': document.dispatchEvent(new CustomEvent('gugu:open-tab', { detail: { name, tab: 'metrics' } })); break;
                default: break;
            }
        });
    });
}

function kpi(label, value) {
    return `<div class="kpi"><span class="kpi-label">${esc(label)}</span><span class="kpi-value" title="${esc(value)}">${esc(value)}</span></div>`;
}

/// 进程列表自带的退出记录只有时间、退出码与是否被自动拉起三项。
function exitTable(exits) {
    const rows = exits.map((exit) => {
        const code = exit.code == null ? '未知' : exit.code;
        const tone = exit.code === 0 ? 'ok' : 'bad';
        return `<tr>
            <td>${esc(fmtDateTime(exit.at))}</td>
            <td class="${exit.code == null ? 'muted' : tone}">${esc(String(code))}</td>
            <td>${exit.auto_restarted ? '已自动拉起' : '未拉起'}</td>
        </tr>`;
    }).join('');
    return `<table class="mini-table">
        <thead><tr><th>时间</th><th>退出码</th><th>自动拉起</th></tr></thead>
        <tbody>${rows}</tbody>
    </table>`;
}
