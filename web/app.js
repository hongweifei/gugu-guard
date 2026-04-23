const API = '/api/v1';
let ws = null;
let selectedProcess = null;
let editingName = null;
let allProcesses = [];
let searchQuery = '';
let API_KEY = localStorage.getItem('gugu_api_key') || '';
let logEntries = [];
let logSearchQuery = '';
let logStreamFilter = 'all';
let logLineHeight = 22;
let logVisibleCount = 0;
let logScrollTop = 0;

async function api(method, path, body) {
    const opts = { method };
    if (body) { opts.headers = { 'Content-Type': 'application/json' }; opts.body = JSON.stringify(body); }
    if (API_KEY) {
        opts.headers = opts.headers || {};
        opts.headers['Authorization'] = `Bearer ${API_KEY}`;
    }
    const resp = await fetch(`${API}${path}`, opts);
    if (resp.status === 401) {
        showLogin();
        throw new Error('Unauthorized');
    }
    return resp.json();
}

function esc(t) { return t.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;'); }
function fmtTime(s) { if (s == null) return '-'; return `${String(Math.floor(s/3600)).padStart(2,'0')}:${String(Math.floor((s%3600)/60)).padStart(2,'0')}:${String(s%60).padStart(2,'0')}`; }

function statusOf(p) {
    const isObj = typeof p.status === 'object' && p.status !== null;
    const raw = isObj ? Object.keys(p.status)[0] : (typeof p.status === 'string' ? p.status : 'stopped');
    const map = { running:['running','运行中'], stopped:['stopped','已停止'], starting:['starting','启动中'], failed:['failed','失败'], restarting:['restarting','重启中'] };
    const m = map[raw] || ['stopped', raw];
    const detail = isObj && raw === 'failed' ? ': ' + (p.status.failed || Object.values(p.status)[0] || '') : '';
    return { key: m[0], label: m[1] + detail };
}

function toast(msg, type = 'info') {
    const el = document.createElement('div');
    el.className = `toast ${type}`;
    el.textContent = msg;
    document.getElementById('toast-container').appendChild(el);
    setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; setTimeout(() => el.remove(), 300); }, 3000);
}

function updateStats(list) {
    const r = list.filter(p => statusOf(p).key === 'running').length;
    const s = list.filter(p => statusOf(p).key === 'stopped').length;
    const f = list.filter(p => statusOf(p).key === 'failed').length;
    document.getElementById('stat-total').textContent = list.length;
    document.getElementById('stat-running').textContent = r;
    document.getElementById('stat-stopped').textContent = s;
    document.getElementById('stat-failed').textContent = f;
}

let prevKeys = [];

function render(force) {
    const grid = document.getElementById('process-grid');
    const list = searchQuery
        ? allProcesses.filter(p => p.name.toLowerCase().includes(searchQuery) || (p.command || '').toLowerCase().includes(searchQuery))
        : allProcesses;

    updateStats(allProcesses);

    const keys = list.map(p => p.name);

    if (!list.length) {
        const isSearch = searchQuery && allProcesses.length > 0;
        grid.innerHTML = `<div class="empty-state">
            <div class="empty-icon"><svg viewBox="0 0 80 80" fill="none" stroke="currentColor" stroke-width="1.5">
                ${isSearch
                    ? '<circle cx="35" cy="35" r="18"/><line x1="48" y1="48" x2="65" y2="65" stroke-width="4" stroke-linecap="round"/>'
                    : '<rect x="10" y="20" width="60" height="40" rx="6"/><circle cx="30" cy="40" r="6"/><circle cx="50" cy="40" r="6"/><line x1="1" y1="40" x2="10" y2="40"/><line x1="70" y1="40" x2="79" y2="40"/>'}
            </svg></div>
            <h3>${isSearch ? '没有匹配的进程' : '暂无受管进程'}</h3>
            <p>${isSearch ? '尝试其他关键词' : '点击上方「添加进程」开始管理你的服务'}</p>
        </div>`;
        prevKeys = [];
        return;
    }

    if (!force && keys.join(',') === prevKeys.join(',')) {
        for (const p of list) {
            const card = grid.querySelector(`[data-name="${CSS.escape(p.name)}"]`);
            if (!card) continue;
            patchCard(card, p);
        }
        return;
    }

    prevKeys = keys;
    grid.innerHTML = list.map((p, i) => buildCard(p, i === 0)).join('');
}

function buildCard(p, isNew) {
    const st = statusOf(p);
    const cmd = p.args?.length ? `${p.command} ${p.args.join(' ')}` : (p.command || '');
    const run = st.key === 'running';
    let hcHtml = '';
    if (p.has_health_check) {
        const cls = p.healthy === true ? 'healthy' : (p.healthy === false ? 'unhealthy' : 'unknown');
        const label = p.healthy === true ? '健康' : (p.healthy === false ? '异常' : '待检');
        hcHtml = `<span class="pcard-tag hc ${cls}" title="健康检查: ${label}">HC ${label}</span>`;
    }
    const urTag = p.unhealthy_restart ? '<span class="pcard-tag ur" title="失败自动重启">AR</span>' : '';
    const hcBtn = p.has_health_check ? `<button class="btn btn-hc" data-action="health" data-name="${esc(p.name)}" title="手动健康检查">检查</button>` : '';
    return `<div class="pcard" data-name="${esc(p.name)}">
        <div class="pcard-head">
            <div class="pcard-name"><span class="pcard-dot ${st.key}"></span>${esc(p.name)}</div>
            <div class="pcard-badges"><span class="pcard-badge ${st.key}">${esc(st.label)}</span>${hcHtml}${urTag}</div>
        </div>
        <div class="pcard-meta">
            <div class="pcard-meta-item"><span class="pcard-meta-label">PID</span><span class="pcard-meta-val">${p.pid || '—'}</span></div>
            <div class="pcard-meta-item"><span class="pcard-meta-label">运行时间</span><span class="pcard-meta-val">${fmtTime(p.uptime_secs)}</span></div>
            <div class="pcard-meta-item pcard-cmd"><span class="pcard-meta-label">命令</span><span class="pcard-meta-val" title="${esc(cmd)}">${esc(cmd)}</span></div>
        </div>
        <div class="pcard-actions">
            <button class="btn act-start" data-action="start" data-name="${esc(p.name)}" ${run?'disabled':''}>启动</button>
            <button class="btn act-stop" data-action="stop" data-name="${esc(p.name)}" ${!run?'disabled':''}>停止</button>
            <button class="btn act-restart" data-action="restart" data-name="${esc(p.name)}">重启</button>
            ${hcBtn}
            <button class="btn act-logs" data-action="logs" data-name="${esc(p.name)}">日志</button>
            <button class="btn" data-action="edit" data-name="${esc(p.name)}">编辑</button>
            <button class="btn act-delete" data-action="remove" data-name="${esc(p.name)}">删除</button>
        </div>
    </div>`;
}

function patchCard(card, p) {
    const st = statusOf(p);
    const run = st.key === 'running';
    const cmd = p.args?.length ? `${p.command} ${p.args.join(' ')}` : (p.command || '');

    card.querySelector('.pcard-dot').className = `pcard-dot ${st.key}`;
    card.querySelector('.pcard-badge').className = `pcard-badge ${st.key}`;
    card.querySelector('.pcard-badge').textContent = st.label;

    const metas = card.querySelectorAll('.pcard-meta-val');
    metas[0].textContent = p.pid || '—';
    metas[1].textContent = fmtTime(p.uptime_secs);
    metas[2].textContent = cmd;
    metas[2].title = cmd;

    const btns = card.querySelectorAll('.pcard-actions .btn');
    btns[0].disabled = run;
    btns[1].disabled = !run;
}

async function doAct(action, name) {
    try {
        const r = await api('POST', `/processes/${encodeURIComponent(name)}/${action}`);
        toast(r.message || '操作成功', 'success');
        if (r.error) toast(r.error, 'error');
    } catch { toast('操作失败', 'error'); }
}

async function doHealthCheck(name) {
    try {
        const r = await api('POST', `/processes/${encodeURIComponent(name)}/health`);
        if (r.healthy) {
            toast('健康检查通过', 'success');
        } else {
            toast('健康检查失败', 'error');
        }
    } catch { toast('健康检查请求失败', 'error'); }
}

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

function openModal(title, submit) {
    document.getElementById('modal-title').textContent = title;
    document.getElementById('btn-submit-form').textContent = submit;
    document.getElementById('modal-backdrop').classList.add('open');
    updatePreview();
}

function closeModal() {
    document.getElementById('modal-backdrop').classList.remove('open');
    document.getElementById('process-form').reset();
    document.getElementById('f-auto-start').checked = true;
    document.getElementById('f-auto-restart').checked = true;
    document.getElementById('args-list').innerHTML = '';
    document.getElementById('env-list').innerHTML = '';
    document.getElementById('f-name').classList.remove('duplicate');
    document.getElementById('f-name-hint').className = 'field-hint';
    document.getElementById('f-name-hint').textContent = '';
    resetHealthCheck();
    editingName = null;
}

window.addArgRow = function(value = '') {
    const div = document.createElement('div');
    div.className = 'dyn-row';
    div.innerHTML = `<input type="text" class="dyn-input arg-input" placeholder="--arg value" value="${esc(value)}"><button type="button" class="btn-icon btn-remove" onclick="this.parentElement.remove();updatePreview()" title="删除"><svg viewBox="0 0 20 20" fill="currentColor" width="14" height="14"><path fill-rule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"/></svg></button>`;
    document.getElementById('args-list').appendChild(div);
    div.querySelector('input').addEventListener('input', updatePreview);
    div.querySelector('input').focus();
};

window.addEnvRow = function(key = '', val = '') {
    const div = document.createElement('div');
    div.className = 'env-row';
    div.innerHTML = `<input type="text" class="dyn-input env-key" placeholder="KEY" value="${esc(key)}"><span class="env-eq">=</span><input type="text" class="dyn-input env-val" placeholder="value" value="${esc(val)}"><button type="button" class="btn-icon btn-remove" onclick="this.parentElement.remove()" title="删除"><svg viewBox="0 0 20 20" fill="currentColor" width="14" height="14"><path fill-rule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"/></svg></button>`;
    document.getElementById('env-list').appendChild(div);
    div.querySelector('.env-key').focus();
};

function collectArgs() {
    return [...document.querySelectorAll('.arg-input')].map(i => i.value.trim()).filter(Boolean);
}
function collectEnv() {
    const env = {};
    document.querySelectorAll('.env-row').forEach(row => {
        const k = row.querySelector('.env-key').value.trim();
        const v = row.querySelector('.env-val').value.trim();
        if (k) env[k] = v;
    });
    return env;
}

window.updatePreview = function() {
    const cmd = document.getElementById('f-command')?.value || '';
    const args = collectArgs();
    const full = [cmd, ...args].filter(Boolean).join(' ');
    document.getElementById('cmd-text').textContent = full || '-';
};

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
    document.getElementById('f-unhealthy-restart').checked = false;
    toggleHcFields();
}

function checkNameDuplicate() {
    const input = document.getElementById('f-name');
    const hint = document.getElementById('f-name-hint');
    const val = input.value.trim();
    input.classList.remove('duplicate');
    hint.className = 'field-hint';
    hint.textContent = '';
    if (!val) return false;
    const isEdit = !!editingName;
    const other = allProcesses.find(p => p.name === val);
    if (other && (!isEdit || val !== editingName)) {
        input.classList.add('duplicate');
        hint.className = 'field-hint error';
        hint.textContent = isEdit ? '此名称已被其他进程使用' : '此名称已存在';
        return true;
    }
    return false;
}

document.getElementById('btn-add').onclick = () => {
    editingName = null;
    document.getElementById('f-name').disabled = false;
    document.getElementById('f-name').value = '';
    openModal('添加进程', '添加');
};
document.getElementById('f-command').addEventListener('input', updatePreview);
document.getElementById('f-name').addEventListener('input', checkNameDuplicate);
document.getElementById('f-hc-type').addEventListener('change', toggleHcFields);

let fsCallback = null;
let fsCurrentPath = '';
let fsCurrentParent = null;
let fsSelectedItem = null;

let fsListEl = null;
let fsActiveEl = null;

async function fsBrowse(path) {
    fsSelectedItem = null;
    fsActiveEl = null;
    if (!fsListEl) fsListEl = document.getElementById('fs-list');
    try {
        const data = await api('GET', `/fs/browse?path=${encodeURIComponent(path)}`);
        fsCurrentPath = data.path || path;
        fsCurrentParent = data.parent || null;
        document.getElementById('fs-path').value = fsCurrentPath;
        if (!data.entries?.length) {
            fsListEl.innerHTML = '<div class="fs-empty">空目录</div>';
            return;
        }
        const parts = [];
        for (let i = 0, len = data.entries.length; i < len; i++) {
            const e = data.entries[i];
            parts.push('<div class="fs-item ', e.is_dir ? 'fs-dir' : 'fs-file', '" data-i="', i, '" title="', esc(e.path), '"><span>', esc(e.name), '</span></div>');
        }
        fsListEl.innerHTML = parts.join('');
        fsListEl._entries = data.entries;
    } catch { toast('无法浏览目录', 'error'); }
}

document.getElementById('fs-list').addEventListener('click', (e) => {
    const item = e.target.closest('.fs-item');
    if (!item) return;
    if (fsActiveEl) fsActiveEl.classList.remove('active');
    item.classList.add('active');
    fsActiveEl = item;
    const entries = item.parentElement._entries;
    const entry = entries[item.dataset.i];
    fsSelectedItem = { path: entry.path, isDir: entry.is_dir };
});

document.getElementById('fs-list').addEventListener('dblclick', (e) => {
    const item = e.target.closest('.fs-item');
    if (!item) return;
    const entries = item.parentElement._entries;
    const entry = entries[item.dataset.i];
    if (entry.is_dir) {
        fsBrowse(entry.path);
    } else {
        fsSelectedItem = { path: entry.path, isDir: false };
        closeFsBrowser();
    }
});

let fsSelectDir = false;

function openFsBrowser(title, confirmText, selectDir, callback) {
    fsCallback = callback;
    fsSelectDir = selectDir;
    fsSelectedItem = null;
    document.getElementById('fs-title').textContent = title;
    document.getElementById('fs-confirm').textContent = confirmText;
    document.getElementById('fs-backdrop').classList.add('open');
    const startPath = document.getElementById('f-dir').value.trim() || '.';
    fsBrowse(startPath);
}

function closeFsBrowser() {
    document.getElementById('fs-backdrop').classList.remove('open');
    if (fsCallback) {
        const path = fsSelectedItem ? fsSelectedItem.path : (fsSelectDir ? fsCurrentPath : null);
        if (path) fsCallback(path);
    }
    fsCallback = null;
}

document.getElementById('btn-pick-cmd').onclick = () => openFsBrowser('选择可执行文件', '选择文件', false, (path) => {
    document.getElementById('f-command').value = path;
    updatePreview();
});
document.getElementById('btn-pick-dir').onclick = () => openFsBrowser('选择工作目录', '选择此目录', true, (path) => {
    document.getElementById('f-dir').value = path;
});
document.getElementById('btn-close-fs').onclick = () => { fsCallback = null; document.getElementById('fs-backdrop').classList.remove('open'); };
document.getElementById('fs-cancel').onclick = () => { fsCallback = null; document.getElementById('fs-backdrop').classList.remove('open'); };
document.getElementById('fs-confirm').onclick = closeFsBrowser;
document.getElementById('fs-go').onclick = () => fsBrowse(document.getElementById('fs-path').value.trim());
document.getElementById('fs-up').onclick = () => {
    if (fsCurrentParent) fsBrowse(fsCurrentParent);
};
document.getElementById('fs-path').onkeydown = (e) => { if (e.key === 'Enter') fsBrowse(e.target.value.trim()); };

window.editProc = async function(name) {
    const p = allProcesses.find(x => x.name === name);
    if (!p) return;
    editingName = name;
    const st = statusOf(p);
    const f = document.getElementById.bind(document);
    f('f-name').value = name;
    f('f-name').disabled = st.key === 'running';
    f('f-command').value = p.command || '';
    f('f-auto-start').checked = p.auto_start ?? true;
    f('f-auto-restart').checked = p.auto_restart ?? true;

    const argsList = document.getElementById('args-list');
    argsList.innerHTML = '';
    if (p.args?.length) {
        p.args.forEach(a => addArgRow(a));
    }

    const envList = document.getElementById('env-list');
    envList.innerHTML = '';

    try {
        const cfg = await api('GET', `/processes/${encodeURIComponent(name)}/config`);
        f('f-dir').value = cfg.working_dir || '';
        f('f-stdout').value = cfg.stdout_log || '';
        f('f-stderr').value = cfg.stderr_log || '';
        f('f-max-restarts').value = cfg.max_restarts ?? 3;
        f('f-restart-delay').value = cfg.restart_delay_secs ?? 5;
        f('f-stop-command').value = cfg.stop_command || '';
        f('f-stop-timeout').value = cfg.stop_timeout_secs ?? 10;
        if (cfg.env && typeof cfg.env === 'object') {
            Object.entries(cfg.env).forEach(([k, v]) => addEnvRow(k, v));
        }
        fillHealthCheck(cfg);
    } catch {
        f('f-dir').value = '';
        f('f-stdout').value = '';
        f('f-stderr').value = '';
        f('f-max-restarts').value = 3;
        f('f-restart-delay').value = 5;
        f('f-stop-timeout').value = 10;
        resetHealthCheck();
    }

    openModal('编辑 — ' + name, '保存');
};

document.getElementById('process-form').onsubmit = async (e) => {
    e.preventDefault();
    if (checkNameDuplicate()) { toast('进程名称重复，请修改', 'error'); return; }
    const newName = document.getElementById('f-name').value.trim();
    if (!newName) { toast('请输入进程名称', 'error'); return; }
    const hcData = collectHealthCheck();
    if (hcData === null) { toast('请完善健康检查配置', 'error'); return; }
    const body = {
        command: document.getElementById('f-command').value,
        args: collectArgs(),
        working_dir: document.getElementById('f-dir').value || null,
        env: collectEnv(),
        auto_start: document.getElementById('f-auto-start').checked,
        auto_restart: document.getElementById('f-auto-restart').checked,
        max_restarts: parseInt(document.getElementById('f-max-restarts').value) || 3,
        restart_delay_secs: parseInt(document.getElementById('f-restart-delay').value) || 5,
        stop_command: document.getElementById('f-stop-command').value.trim() || null,
        stop_timeout_secs: parseInt(document.getElementById('f-stop-timeout').value) || 10,
        health_check: hcData.health_check,
        unhealthy_restart: hcData.unhealthy_restart,
        stdout_log: document.getElementById('f-stdout').value || null,
        stderr_log: document.getElementById('f-stderr').value || null,
        start_now: document.getElementById('f-auto-start').checked,
    };
    try {
        if (editingName) {
            if (newName !== editingName) body.new_name = newName;
            const r = await api('PUT', `/processes/${encodeURIComponent(editingName)}`, body);
            toast(r.message || '已更新', 'success');
        } else {
            const r = await api('POST', `/processes/${encodeURIComponent(newName)}`, body);
            toast(r.message || '已添加', 'success');
        }
        closeModal();
    } catch { toast('操作失败', 'error'); }
};

let pendingRemove = null;
window.askRemove = function(name) {
    pendingRemove = name;
    document.getElementById('confirm-text').textContent = `确定删除「${name}」？运行中的进程将被停止。`;
    document.getElementById('confirm-backdrop').classList.add('open');
};
document.getElementById('btn-confirm-ok').onclick = async () => {
    if (pendingRemove) {
        try {
            const r = await api('DELETE', `/processes/${encodeURIComponent(pendingRemove)}`);
            toast(r.message || '已删除', 'success');
            if (selectedProcess === pendingRemove) closeLogs();
        } catch { toast('删除失败', 'error'); }
        pendingRemove = null;
    }
    document.getElementById('confirm-backdrop').classList.remove('open');
};
document.getElementById('btn-confirm-cancel').onclick = () => {
    pendingRemove = null;
    document.getElementById('confirm-backdrop').classList.remove('open');
};

document.getElementById('btn-refresh-logs').onclick = () => refreshLogs();
document.getElementById('btn-close-logs').onclick = closeLogs;
document.getElementById('log-body').addEventListener('scroll', onLogScroll);
document.getElementById('log-search').addEventListener('input', (e) => {
    logSearchQuery = e.target.value.toLowerCase().trim();
    renderLogBody();
});
document.getElementById('log-stream-filter').addEventListener('change', (e) => {
    logStreamFilter = e.target.value;
    renderLogBody();
});
document.getElementById('btn-clear-logs').onclick = async () => {
    if (!selectedProcess) return;
    try {
        await api('DELETE', `/processes/${encodeURIComponent(selectedProcess)}/logs`);
        logEntries = [];
        renderLogBody();
        toast('日志已清空', 'success');
    } catch { toast('清空日志失败', 'error'); }
};
document.getElementById('btn-download-logs').onclick = () => {
    if (!selectedProcess) return;
    const tokenParam = API_KEY ? `&token=${encodeURIComponent(API_KEY)}` : '';
    const url = `${API}/processes/${encodeURIComponent(selectedProcess)}/logs/download?lines=1000${tokenParam}`;
    const a = document.createElement('a');
    a.href = url;
    a.download = `${selectedProcess}.log`;
    a.click();
};
document.getElementById('btn-close-modal').onclick = closeModal;
document.getElementById('btn-cancel-form').onclick = closeModal;

document.getElementById('search-input').oninput = (e) => {
    searchQuery = e.target.value.toLowerCase().trim();
    render(true);
};

// 事件委托：进程卡片上的按钮
document.getElementById('process-grid').addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn) return;
    const action = btn.dataset.action;
    const name = btn.dataset.name;
    if (!action || !name) return;
    switch (action) {
        case 'start': case 'stop': case 'restart': doAct(action, name); break;
        case 'health': doHealthCheck(name); break;
        case 'logs': showLogs(name); break;
        case 'edit': editProc(name); break;
        case 'remove': askRemove(name); break;
    }
});

document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        if (document.getElementById('log-overlay').classList.contains('open')) closeLogs();
        else if (document.getElementById('fs-backdrop').classList.contains('open')) { fsCallback = null; document.getElementById('fs-backdrop').classList.remove('open'); }
        else if (document.getElementById('modal-backdrop').classList.contains('open')) closeModal();
        else if (document.getElementById('confirm-backdrop').classList.contains('open')) { pendingRemove = null; document.getElementById('confirm-backdrop').classList.remove('open'); }
        else if (document.getElementById('login-backdrop').classList.contains('open')) { /* don't close login */ }
    }
});

function connectWS() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const tokenParam = API_KEY ? `?token=${encodeURIComponent(API_KEY)}` : '';
    ws = new WebSocket(`${proto}//${location.host}${API}/ws${tokenParam}`);
    ws.onmessage = (ev) => {
        try {
            const d = JSON.parse(ev.data);
            if (d.type === 'status') {
                allProcesses = d.processes || [];
                render();
            } else if (d.type === 'logs') {
                if (d.entries?.length) {
                    appendLogEntries(d.entries);
                }
            }
        } catch {}
    };
    ws.onclose = () => setTimeout(connectWS, 3000);
    ws.onerror = () => ws.close();
}

function showLogin() {
    document.getElementById('login-backdrop').classList.add('open');
    setTimeout(() => document.getElementById('login-key').focus(), 100);
}

window.toggleLoginVis = function() {
    const inp = document.getElementById('login-key');
    const open = document.querySelector('.login-toggle-vis .eye-open');
    const closed = document.querySelector('.login-toggle-vis .eye-closed');
    if (inp.type === 'password') {
        inp.type = 'text';
        open.style.display = 'none';
        closed.style.display = 'block';
    } else {
        inp.type = 'password';
        open.style.display = 'block';
        closed.style.display = 'none';
    }
};

document.getElementById('btn-login').onclick = async () => {
    const errEl = document.getElementById('login-error');
    const btn = document.getElementById('btn-login');
    errEl.textContent = '';
    document.getElementById('login-key').classList.remove('input-error');
    const key = document.getElementById('login-key').value.trim();
    if (!key) { errEl.textContent = '请输入 API Key'; return; }
    API_KEY = key;
    btn.disabled = true;
    btn.textContent = '验证中...';
    try {
        const resp = await fetch(`${API}/processes`, {
            headers: { 'Authorization': `Bearer ${key}` }
        });
        if (!resp.ok) {
            API_KEY = '';
            errEl.textContent = 'API Key 无效，请检查后重试';
            document.getElementById('login-key').classList.add('input-error');
            document.getElementById('login-key').focus();
            return;
        }
        allProcesses = await resp.json();
        localStorage.setItem('gugu_api_key', key);
        document.getElementById('login-backdrop').classList.remove('open');
        render(true);
        connectWS();
    } catch {
        API_KEY = '';
        errEl.textContent = '无法连接到服务';
    } finally {
        btn.disabled = false;
        btn.textContent = '认证并登录';
    }
};

document.getElementById('login-key').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') document.getElementById('btn-login').click();
});

async function init() {
    try {
        const opts = {};
        if (API_KEY) opts.headers = { 'Authorization': `Bearer ${API_KEY}` };
        const resp = await fetch(`${API}/processes`, opts);
        if (resp.status === 401) {
            showLogin();
            return;
        }
        allProcesses = await resp.json();
        render(true);
    } catch {
        document.getElementById('process-grid').innerHTML = '<div class="empty-state"><h3>连接失败</h3><p>无法连接到守护进程</p></div>';
    }
    connectWS();
}
init();
