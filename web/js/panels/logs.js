// 日志面板：实时缓冲 + 历史文件检索 + 虚拟滚动。
// 实时增量来自 WS（所有进程的日志都在同一条通道上，按 process_name 过滤）。

import { $ } from '../ui/dom.js';
import { API_BASE, api, apiPath } from '../core/api.js';
import { esc, levelRank, normStream } from '../core/format.js';
import { onLogEntries, subscribeProcess } from '../core/live.js';
import { state, subscribe } from '../core/state.js';
import { toast } from '../ui/toast.js';

const LINE_HEIGHT = 22;
const MAX_ENTRIES = 2000;
const KEEP_ENTRIES = 1500;

let target = null;
let entries = [];
let mode = 'live';
let query = '';
let stream = 'all';
let level = 'all';
let active = false;

export function initLogs() {
    bindToolbar();

    onLogEntries((batch) => {
        if (!active || !target || mode !== 'live') return;
        for (const entry of batch) {
            if (entry.process_name && entry.process_name !== target) continue;
            entries.push(mapEntry(entry, false));
        }
        if (entries.length > MAX_ENTRIES) entries = entries.slice(-KEEP_ENTRIES);
        render();
        scrollToBottom();
    });

    subscribe((reason) => {
        if (reason === 'reset' && active) {
            entries = [];
            render();
        }
    });

    $('log-body').addEventListener('scroll', () => {
        requestAnimationFrame(render);
    });
}

/// 抽屉切到日志页时调用；`name` 为空表示关闭。
export async function logsActivate(name, tabActive) {
    active = tabActive;
    if (!tabActive) return;

    if (target !== name) {
        target = name;
        entries = [];
        mode = 'live';
        query = '';
        stream = 'all';
        level = 'all';
        $('log-search').value = '';
        $('log-stream').value = 'all';
        $('log-level').value = 'all';
        setStatus('');
        render();
        subscribeProcess(name);
        await refresh();
        return;
    }
    requestAnimationFrame(render);
}

async function refresh() {
    if (!target) return;
    mode = 'live';
    setStatus('');
    const params = filterParams();
    params.set('lines', '1000');
    try {
        const data = await api('GET', `/processes/${encodeURIComponent(target)}/logs?${params}`);
        if (!Array.isArray(data)) {
            toast(data?.error || '读取日志失败', 'error');
            return;
        }
        entries = data.map((entry) => mapEntry(entry, false));
        render();
        scrollToBottom();
    } catch (error) {
        if (error.message === 'Unauthorized') return;
        toast(error.message || '读取日志失败', 'error');
    }
}

async function searchHistory() {
    if (!target) return;
    const params = filterParams();
    params.set('limit', '300');
    params.set('budget_mb', '4');
    try {
        const data = await api('GET', `/processes/${encodeURIComponent(target)}/logs/search?${params}`);
        if (!data || !Array.isArray(data.hits)) {
            toast(data?.error || '检索历史日志失败', 'error');
            return;
        }
        mode = 'file';
        entries = data.hits.map((entry) => mapEntry(entry, true));
        const files = (data.files || []).join('、') || '无';
        const scanned = data.scanned_bytes >= 1048576
            ? `${(data.scanned_bytes / 1048576).toFixed(1)} MB`
            : `${Math.ceil((data.scanned_bytes || 0) / 1024)} KB`;
        const truncated = data.truncated ? '，已截断（保留最近一段）' : '';
        setStatus(
            `历史文件：命中 <strong>${data.hits.length}</strong> / ${data.matched ?? data.hits.length} 条，`
            + `扫描 ${data.scanned_lines ?? 0} 行 / ${scanned}${truncated}；文件 ${files}`
            + ' <button type="button" class="btn btn-sm" data-act="live">回到实时</button>',
            true,
        );
        render();
        $('log-body').scrollTop = 0;
    } catch (error) {
        if (error.message === 'Unauthorized') return;
        toast(error.message || '检索历史日志失败', 'error');
    }
}

function bindToolbar() {
    $('log-search').addEventListener('input', (event) => {
        query = event.target.value.toLowerCase().trim();
        render();
    });
    $('log-level').addEventListener('change', (event) => {
        level = event.target.value;
        render();
    });
    $('log-stream').addEventListener('change', (event) => {
        stream = event.target.value;
        render();
    });
    $('log-history').addEventListener('click', searchHistory);
    $('log-refresh').addEventListener('click', refresh);
    $('log-clear').addEventListener('click', clearLogs);
    $('log-download').addEventListener('click', (event) => {
        if (!target) {
            event.preventDefault();
            return;
        }
        const token = state.apiKey ? `&token=${encodeURIComponent(state.apiKey)}` : '';
        const path = apiPath(`/processes/${encodeURIComponent(target)}/logs/download`);
        $('log-download').href = `${API_BASE}${path}?lines=1000${token}`;
        $('log-download').setAttribute('download', `${target}.log`);
    });
    $('log-status').addEventListener('click', (event) => {
        if (event.target.closest('[data-act="live"]')) refresh();
    });
}

async function clearLogs() {
    if (!target) return;
    try {
        await api('DELETE', `/processes/${encodeURIComponent(target)}/logs`);
        entries = [];
        render();
        toast('日志已清空', 'success');
    } catch (error) {
        toast(error.message || '清空日志失败', 'error');
    }
}

function filterParams() {
    const params = new URLSearchParams();
    if (level !== 'all') params.set('level', level);
    if (query) params.set('q', query);
    if (stream !== 'all') params.set('stream', stream);
    return params;
}

function mapEntry(entry, useParsedTime) {
    const parsed = entry.parsed || null;
    return {
        timestamp: useParsedTime ? (parsed?.at || null) : (entry.timestamp || null),
        stream: normStream(entry.stream),
        line: entry.line || '',
        level: parsed?.level || null,
        message: parsed?.message || null,
        fields: parsed?.fields || null,
    };
}

function visibleEntries() {
    const minRank = level === 'all' ? -1 : levelRank(level);
    return entries.filter((entry) => {
        if (stream !== 'all' && entry.stream !== stream) return false;
        if (minRank >= 0 && levelRank(entry.level) < minRank) return false;
        if (query && !entry.line.toLowerCase().includes(query)) return false;
        return true;
    });
}

function render() {
    const body = $('log-body');
    const list = visibleEntries();
    $('log-count').textContent = list.length ? `${list.length} 行` : '';

    if (!list.length) {
        const reason = entries.length
            ? '没有匹配的日志'
            : (mode === 'file' ? '历史文件里没有命中' : '暂无日志输出');
        body.innerHTML = `<div class="chart-empty" style="margin:12px">${reason}</div>`;
        return;
    }

    const viewport = body.clientHeight || 320;
    const scrollTop = body.scrollTop;
    const visibleCount = Math.ceil(viewport / LINE_HEIGHT) + 2;
    const start = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - 1);
    const end = Math.min(list.length, start + visibleCount + 2);

    const topPad = start * LINE_HEIGHT;
    const bottomPad = (list.length - end) * LINE_HEIGHT;

    const parts = [];
    for (let i = start; i < end; i += 1) parts.push(lineHtml(list[i]));
    body.innerHTML = `<div style="height:${topPad}px"></div>${parts.join('')}<div style="height:${bottomPad}px"></div>`;
}

function lineHtml(entry) {
    const time = mode === 'file'
        ? (entry.timestamp ? formatFileTime(entry.timestamp) : '—')
        : formatLiveTime(entry.timestamp);
    const highlight = query && entry.line.toLowerCase().includes(query) ? ' highlight' : '';
    const streamTag = entry.stream === 'stdout' ? 'out' : 'err';
    const levelTag = entry.level
        ? `<span class="log-level ${entry.level}">${entry.level.toUpperCase()}</span>`
        : '';
    const fields = fieldsHtml(entry.fields);
    return `<div class="log-line${highlight}"><span class="log-ts">${time}</span>`
        + `<span class="log-tag ${streamTag}">${entry.stream === 'stdout' ? 'OUT' : 'ERR'}</span>`
        + `${levelTag}<span class="log-msg">${esc(entry.message || entry.line)}</span>${fields}</div>`;
}

function fieldsHtml(fields) {
    if (!fields) return '';
    const keys = Object.keys(fields);
    if (!keys.length) return '';
    const shown = keys.slice(0, 5);
    let html = '<span class="log-fields">';
    for (const key of shown) {
        html += `<span class="log-field" title="${esc(key)} = ${esc(fields[key])}"><b>${esc(key)}</b>=${esc(fields[key])}</span>`;
    }
    if (keys.length > shown.length) html += `<span class="log-field">+${keys.length - shown.length}</span>`;
    return `${html}</span>`;
}

function formatLiveTime(value) {
    if (!value) return '—';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return '—';
    const pad = (n) => String(n).padStart(2, '0');
    return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function formatFileTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return '—';
    const pad = (n) => String(n).padStart(2, '0');
    return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}


function scrollToBottom() {
    if (!$('log-follow').checked) return;
    const body = $('log-body');
    requestAnimationFrame(() => {
        body.scrollTop = body.scrollHeight;
    });
}

function setStatus(html, history = false) {
    const node = $('log-status');
    if (!html) {
        node.hidden = true;
        node.innerHTML = '';
        node.classList.remove('history');
        return;
    }
    node.hidden = false;
    node.classList.toggle('history', history);
    node.innerHTML = html;
}

