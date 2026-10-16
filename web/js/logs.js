// 日志查看器：打开/刷新/虚拟滚动渲染/实时追加。

async function showLogs(name) {
    selectedProcess = name;
    logEntries = [];
    logSearchQuery = '';
    logStreamFilter = 'all';
    document.getElementById('log-process-name').textContent = name;
    document.getElementById('log-search').value = '';
    document.getElementById('log-stream-filter').value = 'all';
    document.getElementById('log-overlay').classList.add('open');
    if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'subscribe', process: name }));
    }
    await refreshLogs();
}

async function refreshLogs() {
    if (!selectedProcess) return;
    const data = await api('GET', `/processes/${encodeURIComponent(selectedProcess)}/logs?lines=1000`);
    if (!data?.length) {
        logEntries = [];
        renderLogBody();
        return;
    }
    logEntries = data.map(e => ({
        timestamp: e.timestamp,
        stream: typeof e.stream === 'string' ? e.stream.toLowerCase() : (e.stream.Stdout ? 'stdout' : 'stderr'),
        line: e.line,
    }));
    renderLogBody();
    if (document.getElementById('log-autoscroll').checked) {
        const el = document.getElementById('log-body');
        requestAnimationFrame(() => el.scrollTop = el.scrollHeight);
    }
}

function getFilteredEntries() {
    return logEntries.filter(e => {
        if (logStreamFilter !== 'all' && e.stream !== logStreamFilter) return false;
        if (logSearchQuery && !e.line.toLowerCase().includes(logSearchQuery)) return false;
        return true;
    });
}

function renderLogBody() {
    const el = document.getElementById('log-body');
    const filtered = getFilteredEntries();
    if (!filtered.length) {
        el.innerHTML = '<div class="log-empty">' + (logEntries.length ? '没有匹配的日志' : '暂无日志输出') + '</div>';
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
        const e = filtered[i];
        const t = new Date(e.timestamp).toLocaleTimeString();
        const isOut = e.stream === 'stdout';
        const highlight = logSearchQuery && e.line.toLowerCase().includes(logSearchQuery) ? ' highlight' : '';
        parts.push(`<div class="log-line${highlight}"><span class="log-ts">${t}</span><span class="log-tag ${isOut?'out':'err'}">${isOut?'OUT':'ERR'}</span><span class="log-msg">${esc(e.line)}</span></div>`);
    }
    el.innerHTML = `<div style="height:${topPad}px"></div>${parts.join('')}<div style="height:${bottomPad}px"></div>`;
}

function onLogScroll() {
    requestAnimationFrame(renderLogBody);
}

function closeLogs() {
    selectedProcess = null;
    logEntries = [];
    document.getElementById('log-overlay').classList.remove('open');
}

function appendLogEntries(entries) {
    if (!selectedProcess) return;
    for (const e of entries) {
        const procName = e.process_name;
        if (procName && procName !== selectedProcess) continue;
        const stream = typeof e.stream === 'string' ? e.stream.toLowerCase() : (e.stream.Stdout ? 'stdout' : 'stderr');
        logEntries.push({
            timestamp: e.timestamp,
            stream,
            line: e.line,
        });
    }
    if (logEntries.length > 2000) {
        logEntries = logEntries.slice(logEntries.length - 1500);
    }
    renderLogBody();
    if (document.getElementById('log-autoscroll').checked) {
        const el = document.getElementById('log-body');
        requestAnimationFrame(() => el.scrollTop = el.scrollHeight);
    }
}
