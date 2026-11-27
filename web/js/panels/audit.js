// 操作审计视图：按动作、时间范围、关键字与结果筛选写操作记录。

import { $, debounce } from '../ui/dom.js';
import { api } from '../core/api.js';
import { auditActor, esc, fmtDateTime } from '../core/format.js';
import { toast } from '../ui/toast.js';

let seq = 0;
let loaded = false;

export function initAudit() {
    $('audit-action').addEventListener('change', reload);
    $('audit-range').addEventListener('change', reload);
    $('audit-failed').addEventListener('change', reload);
    $('audit-search').addEventListener('input', debounce(reload, 300));
    $('btn-audit-refresh').addEventListener('click', reload);
}

/// 切换到审计视图时调用：首次进入自动加载，之后每次进入都刷新。
export function auditActivate() {
    if (!loaded) {
        loaded = true;
        reload();
        return;
    }
    reload();
}

function queryString() {
    const params = new URLSearchParams();
    params.set('limit', '200');
    const action = $('audit-action').value;
    if (action) params.set('action', action);
    const range = $('audit-range').value;
    if (range !== '0') params.set('since_minutes', range);
    const query = $('audit-search').value.trim();
    if (query) params.set('q', query);
    if ($('audit-failed').checked) params.set('ok', 'false');
    return params.toString();
}

async function reload() {
    const current = ++seq;
    try {
        const data = await api('GET', `/audit?${queryString()}`);
        if (current !== seq) return;
        render(data);
    } catch (error) {
        if (current !== seq) return;
        if (error.message === 'Unauthorized') return;
        toast(error.message || '读取审计记录失败', 'error');
    }
}

function render(data) {
    const note = $('audit-note');
    note.textContent = `记录保留 ${data?.retention_days ?? 90} 天`;
    if (data?.file) note.title = data.file;

    const entries = data?.entries || [];
    const body = $('atable-body');
    const table = $('atable');
    const empty = $('audit-empty');

    if (!entries.length) {
        table.hidden = true;
        empty.hidden = false;
        empty.innerHTML = '<h3>暂无操作记录</h3><p>只有改变状态的写操作（启停、增删改、重载、停止守护）才会记入审计</p>';
        return;
    }

    table.hidden = false;
    empty.hidden = true;
    body.innerHTML = entries.map(rowHtml).join('');
}

function rowHtml(entry) {
    const ok = entry.ok;
    const result = ok
        ? '<span class="badge badge-ok">成功</span>'
        : `<span class="badge badge-bad">失败 ${esc(entry.status ?? '')}</span>`;
    return `<tr class="${ok ? '' : 'fail'}">
        <td class="at-time">${esc(fmtDateTime(entry.at))}</td>
        <td class="at-action"><span class="at-action-label">${esc(entry.label)}</span><code>${esc(entry.action)}</code></td>
        <td class="at-target">${entry.target ? esc(entry.target) : '—'}</td>
        <td>${result}</td>
        <td class="at-client">${esc(entry.client || '—')}</td>
        <td class="at-actor">${esc(auditActor(entry))}</td>
        <td class="at-duration">${entry.duration_ms < 1 ? '<1 ms' : `${entry.duration_ms} ms`}</td>
        <td class="at-detail" title="${esc(entry.detail || '')}">${entry.detail ? esc(entry.detail) : '—'}</td>
    </tr>`;
}
