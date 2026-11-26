// 日志查看器：实时读取、历史文件检索、虚拟滚动渲染、实时追加。

const LOG_LEVEL_RANK = { trace: 0, debug: 1, info: 2, warn: 3, error: 4, fatal: 5 };

function levelRank(level) {
    return level && LOG_LEVEL_RANK[level] != null ? LOG_LEVEL_RANK[level] : -1;
}

function normStream(stream) {
    if (typeof stream === 'string') return stream.toLowerCase();
    return stream && stream.Stdout ? 'stdout' : 'stderr';
}

// 实时行的时间取采集时刻；历史文件的行内时间可能缺失，缺了就留空，不拿别的时间冒充。
function mapLogEntry(e, useParsedTime) {
    const parsed = e.parsed || null;
    return {
        timestamp: useParsedTime ? (parsed && parsed.at) || null : (e.timestamp || null),
        stream: normStream(e.stream),
        line: e.line,
        level: parsed && parsed.level ? parsed.level : null,
        message: parsed && parsed.message ? parsed.message : null,
        fields: parsed && parsed.fields ? parsed.fields : null,
    };
}

function logFilterParams() {
    const params = new URLSearchParams();
    if (logLevelFilter !== 'all') params.set('level', logLevelFilter);
    if (logSearchQuery) params.set('q', logSearchQuery);
    if (logStreamFilter !== 'all') params.set('stream', logStreamFilter);
    return params;
}

function setLogStatus(html, history) {
    const el = document.getElementById('log-status');
    if (!html) {
        el.hidden = true;
        el.innerHTML = '';
        el.classList.remove('history');
        return;
    }
    el.hidden = false;
    el.classList.toggle('history', !!history);
    el.innerHTML = html;
}

async function showLogs(name) {
    selectedProcess = name;
    logEntries = [];
    logSearchQuery = '';
    logStreamFilter = 'all';
    logLevelFilter = 'all';
    logMode = 'live';
    document.getElementById('log-process-name').textContent = name;
    document.getElementById('log-search').value = '';
    document.getElementById('log-stream-filter').value = 'all';
    document.getElementById('log-level-filter').value = 'all';
    document.getElementById('log-overlay').classList.add('open');
    if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'subscribe', process: name }));
    }
    await refreshLogs();
}

async function refreshLogs() {
    if (!selectedProcess) return;
    logMode = 'live';
    setLogStatus('');
    const params = logFilterParams();
    params.set('lines', '1000');
    const data = await api('GET', `/processes/${encodeURIComponent(selectedProcess)}/logs?${params}`);
    if (data && !Array.isArray(data)) {
        toast(data.error || '读取日志失败', 'error');
        return;
    }
    logEntries = (data || []).map(e => mapLogEntry(e, false));
    renderLogBody();
    scrollLogToBottom();
}

async function searchHistoryLogs() {
    if (!selectedProcess) return;
    const params = logFilterParams();
    params.set('limit', '300');
    params.set('budget_mb', '4');
    const data = await api('GET', `/processes/${encodeURIComponent(selectedProcess)}/logs/search?${params}`);
    if (data && data.error) {
        toast(data.error, 'error');
        return;
    }
    if (!data || !Array.isArray(data.hits)) {
        toast('检索历史日志失败', 'error');
        return;
    }
    logMode = 'file';
    logEntries = data.hits.map(e => mapLogEntry(e, true));
    const files = (data.files || []).join('、') || '无';
    const scanned = data.scanned_bytes >= 1048576
        ? `${(data.scanned_bytes / 1048576).toFixed(1)} MB`
        : `${Math.ceil(data.scanned_bytes / 1024)} KB`;
    const truncated = data.truncated ? '，结果被截断（只保留最近一段末尾）' : '';
    setLogStatus(
        `历史文件：命中 <strong>${data.hits.length}</strong> / ${data.matched} 条，扫描 ${data.scanned_lines} 行 / ${scanned}${truncated}；文件 ${esc(files)}`
        + ' <button class="btn btn-sm" data-act="live">回到实时</button>',
        true,
    );
    renderLogBody();
    const el = document.getElementById('log-body');
    el.scrollTop = 0;
}

function getFilteredEntries() {
    const minRank = logLevelFilter === 'all' ? -1 : (LOG_LEVEL_RANK[logLevelFilter] ?? -1);
    return logEntries.filter(e => {
        if (logStreamFilter !== 'all' && e.stream !== logStreamFilter) return false;
        if (minRank >= 0 && levelRank(e.level) < minRank) return false;
        if (logSearchQuery && !e.line.toLowerCase().includes(logSearchQuery)) return false;
        return true;
    });
}

function logLineHtml(e) {
    const t = logMode === 'file' ? formatLogTime(e.timestamp) : new Date(e.timestamp).toLocaleTimeString();
    const isOut = e.stream === 'stdout';
    const highlight = logSearchQuery && e.line.toLowerCase().includes(logSearchQuery) ? ' highlight' : '';
    const level = e.level ? `<span class="log-level-tag ${e.level}">${e.level.toUpperCase()}</span>` : '';
    const text = e.message || e.line;
    let fieldsHtml = '';
    if (e.fields) {
        const keys = Object.keys(e.fields);
        const shown = keys.slice(0, 5);
        fieldsHtml = '<span class="log-fields">' + shown.map(k =>
            `<span class="log-field" title="${esc(k)} = ${esc(e.fields[k])}"><b>${esc(k)}</b>=${esc(e.fields[k])}</span>`
        ).join('');
        if (keys.length > shown.length) fieldsHtml += `<span class="log-field">+${keys.length - shown.length}</span>`;
        fieldsHtml += '</span>';
    }
    return `<div class="log-line${highlight}"><span class="log-ts">${t}</span><span class="log-tag ${isOut ? 'out' : 'err'}">${isOut ? 'OUT' : 'ERR'}</span>${level}<span class="log-msg">${esc(text)}</span>${fieldsHtml}</div>`;
}

function formatLogTime(value) {
    if (!value) return '—';
    const d = new Date(value);
    const pad = n => String(n).padStart(2, '0');
    return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function scrollLogToBottom() {
    if (!document.getElementById('log-autoscroll').checked) return;
    const el = document.getElementById('log-body');
    requestAnimationFrame(() => el.scrollTop = el.scrollHeight);
}

function renderLogBody() {
    const el = document.getElementById('log-body');
    const filtered = getFilteredEntries();
    if (!filtered.length) {
        const reason = logEntries.length
            ? '没有匹配的日志'
            : (logMode === 'file' ? '历史文件里没有命中' : '暂无日志输出');
        el.innerHTML = '<div class="log-empty">' + reason + '</div>';
        el.style.overflowY = 'auto';
        return;
    }
    el.style.overflowY = 'auto';

    const containerH = el.clientHeight;
    logVisibleCount = Math.ceil(containerH / logLineHeight) + 2;
    logScrollTop = el.scrollTop;
    const startIdx = Math.max(0, Math.floor(logScrollTop / logLineHeight) - 1);
    const endIdx = Math.min(filtered.length, startIdx + logVisibleCount + 2);

    const topPad = startIdx * logLineHeight;
    const bottomPad = (filtered.length - endIdx) * logLineHeight;

    const parts = [];
    for (let i = startIdx; i < endIdx; i++) {
        parts.push(logLineHtml(filtered[i]));
    }
    el.innerHTML = `<div style="height:${topPad}px"></div>${parts.join('')}<div style="height:${bottomPad}px"></div>`;
}

function onLogScroll() {
    requestAnimationFrame(renderLogBody);
}

function closeLogs() {
    selectedProcess = null;
    logEntries = [];
    logMode = 'live';
    setLogStatus('');
    document.getElementById('log-overlay').classList.remove('open');
}

function appendLogEntries(entries) {
    if (!selectedProcess || logMode !== 'live') return;
    for (const e of entries) {
        const procName = e.process_name;
        if (procName && procName !== selectedProcess) continue;
        logEntries.push(mapLogEntry(e, false));
    }
    if (logEntries.length > 2000) {
        logEntries = logEntries.slice(logEntries.length - 1500);
    }
    renderLogBody();
    scrollLogToBottom();
}
