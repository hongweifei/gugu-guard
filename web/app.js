const API = '/api/v1';
let ws = null;
let selectedProcess = null;
let editingName = null;
let allProcesses = [];
let searchQuery = '';

async function api(method, path, body) {
    const opts = { method };
    if (body) { opts.headers = { 'Content-Type': 'application/json' }; opts.body = JSON.stringify(body); }
    const resp = await fetch(`${API}${path}`, opts);
    return resp.json();
}

function esc(t) { const d = document.createElement('div'); d.textContent = t; return d.innerHTML; }
function fmtTime(s) { if (s == null) return '-'; return `${String(Math.floor(s/3600)).padStart(2,'0')}:${String(Math.floor((s%3600)/60)).padStart(2,'0')}:${String(s%60).padStart(2,'0')}`; }

function statusOf(p) {
    const isObj = typeof p.status === 'object' && p.status !== null;
    const raw = isObj ? Object.keys(p.status)[0] : p.status;
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
    return `<div class="pcard" data-name="${esc(p.name)}">
        <div class="pcard-head">
            <div class="pcard-name"><span class="pcard-dot ${st.key}"></span>${esc(p.name)}</div>
            <span class="pcard-badge ${st.key}">${esc(st.label)}</span>
        </div>
        <div class="pcard-meta">
            <div class="pcard-meta-item"><span class="pcard-meta-label">PID</span><span class="pcard-meta-val">${p.pid || '—'}</span></div>
            <div class="pcard-meta-item"><span class="pcard-meta-label">运行时间</span><span class="pcard-meta-val">${fmtTime(p.uptime_secs)}</span></div>
            <div class="pcard-meta-item pcard-cmd"><span class="pcard-meta-label">命令</span><span class="pcard-meta-val" title="${esc(cmd)}">${esc(cmd)}</span></div>
        </div>
        <div class="pcard-actions">
            <button class="btn act-start" onclick="doAct('start','${p.name}')" ${run?'disabled':''}>启动</button>
            <button class="btn act-stop" onclick="doAct('stop','${p.name}')" ${!run?'disabled':''}>停止</button>
            <button class="btn act-restart" onclick="doAct('restart','${p.name}')">重启</button>
            <button class="btn act-logs" onclick="showLogs('${p.name}')">日志</button>
            <button class="btn" onclick="editProc('${p.name}')">编辑</button>
            <button class="btn act-delete" onclick="askRemove('${p.name}')">删除</button>
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

/* ── Logs ── */
async function showLogs(name) {
    selectedProcess = name;
    document.getElementById('log-process-name').textContent = name;
    document.getElementById('log-overlay').classList.add('open');
    refreshLogs();
}
async function refreshLogs() {
    if (!selectedProcess) return;
    const data = await api('GET', `/processes/${encodeURIComponent(selectedProcess)}/logs?lines=300`);
    const el = document.getElementById('log-body');
    if (!data?.length) { el.innerHTML = '<div class="log-empty">暂无日志输出</div>'; return; }
    el.innerHTML = data.map(e => {
        const t = new Date(e.timestamp).toLocaleTimeString();
        const s = typeof e.stream === 'string' ? e.stream.toLowerCase() : (e.stream.Stdout ? 'stdout' : 'stderr');
        return `<div class="log-line"><span class="log-ts">${t}</span><span class="log-tag ${s==='stdout'?'out':'err'}">${s==='stdout'?'OUT':'ERR'}</span><span class="log-msg">${esc(e.line)}</span></div>`;
    }).join('');
    if (document.getElementById('log-autoscroll').checked) el.scrollTop = el.scrollHeight;
}
function closeLogs() { selectedProcess = null; document.getElementById('log-overlay').classList.remove('open'); }

/* ── Modal ── */
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

/* ── File Browser ── */
let fsCallback = null;
let fsCurrentPath = '';
let fsSelectedItem = null;

async function fsBrowse(path) {
    fsSelectedItem = null;
    try {
        const data = await api('GET', `/fs/browse?path=${encodeURIComponent(path)}`);
        fsCurrentPath = data.path || path;
        document.getElementById('fs-path').value = fsCurrentPath;
        const list = document.getElementById('fs-list');
        if (!data.entries?.length) {
            list.innerHTML = '<div class="fs-empty">空目录</div>';
            return;
        }
        list.innerHTML = data.entries.map(e =>
            `<div class="fs-item${e.is_dir ? ' fs-dir' : ' fs-file'}" data-path="${esc(e.path)}" data-dir="${e.is_dir}" title="${esc(e.path)}">
                <svg class="fs-icon" viewBox="0 0 20 20" fill="currentColor"><path d="${e.is_dir
                    ? 'M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z'
                    : 'M4 4a2 2 0 012-2h4.586A2 2 0 0112 2.586L15.414 6A2 2 0 0116 7.414V16a2 2 0 01-2 2H6a2 2 0 01-2-2V4z'}"/></svg>
                <span>${esc(e.name)}</span>
            </div>`
        ).join('');
        list.querySelectorAll('.fs-item').forEach(el => {
            el.onclick = () => {
                list.querySelectorAll('.fs-item.active').forEach(a => a.classList.remove('active'));
                el.classList.add('active');
                fsSelectedItem = { path: el.dataset.path, isDir: el.dataset.dir === 'true' };
            };
            el.ondblclick = () => {
                if (el.dataset.dir === 'true') fsBrowse(el.dataset.path);
                else { fsSelectedItem = { path: el.dataset.path, isDir: false }; closeFsBrowser(); }
            };
        });
    } catch { toast('无法浏览目录', 'error'); }
}

function openFsBrowser(title, confirmText, selectDir, callback) {
    fsCallback = callback;
    fsSelectedItem = null;
    document.getElementById('fs-title').textContent = title;
    document.getElementById('fs-confirm').textContent = confirmText;
    document.getElementById('fs-backdrop').classList.add('open');
    const startPath = document.getElementById('f-dir').value.trim() || '.';
    fsBrowse(startPath);
}

function closeFsBrowser() {
    document.getElementById('fs-backdrop').classList.remove('open');
    if (fsCallback && fsSelectedItem) fsCallback(fsSelectedItem.path);
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
    const parent = fsCurrentPath.replace(/[\\\/][^\\\/]+[\\\/]?$/, '');
    if (parent) fsBrowse(parent);
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
        if (cfg.env && typeof cfg.env === 'object') {
            Object.entries(cfg.env).forEach(([k, v]) => addEnvRow(k, v));
        }
    } catch {
        f('f-dir').value = '';
        f('f-stdout').value = '';
        f('f-stderr').value = '';
        f('f-max-restarts').value = 3;
        f('f-restart-delay').value = 5;
    }

    openModal('编辑 — ' + name, '保存');
};

document.getElementById('process-form').onsubmit = async (e) => {
    e.preventDefault();
    if (checkNameDuplicate()) { toast('进程名称重复，请修改', 'error'); return; }
    const newName = document.getElementById('f-name').value.trim();
    if (!newName) { toast('请输入进程名称', 'error'); return; }
    const body = {
        command: document.getElementById('f-command').value,
        args: collectArgs(),
        working_dir: document.getElementById('f-dir').value || null,
        env: collectEnv(),
        auto_start: document.getElementById('f-auto-start').checked,
        auto_restart: document.getElementById('f-auto-restart').checked,
        max_restarts: parseInt(document.getElementById('f-max-restarts').value) || 3,
        restart_delay_secs: parseInt(document.getElementById('f-restart-delay').value) || 5,
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

/* ── Confirm ── */
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

/* ── Event bindings ── */
document.getElementById('btn-refresh-logs').onclick = refreshLogs;
document.getElementById('btn-close-logs').onclick = closeLogs;
document.getElementById('btn-close-modal').onclick = closeModal;
document.getElementById('btn-cancel-form').onclick = closeModal;

document.getElementById('search-input').oninput = (e) => {
    searchQuery = e.target.value.toLowerCase().trim();
    render(true);
};

document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        if (document.getElementById('log-overlay').classList.contains('open')) closeLogs();
        else if (document.getElementById('fs-backdrop').classList.contains('open')) { fsCallback = null; document.getElementById('fs-backdrop').classList.remove('open'); }
        else if (document.getElementById('modal-backdrop').classList.contains('open')) closeModal();
        else if (document.getElementById('confirm-backdrop').classList.contains('open')) { pendingRemove = null; document.getElementById('confirm-backdrop').classList.remove('open'); }
    }
});

/* ── WebSocket ── */
function connectWS() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}${API}/ws`);
    ws.onmessage = (ev) => {
        try {
            const d = JSON.parse(ev.data);
            if (d.type === 'status') {
                allProcesses = d.processes || [];
                render();
                if (selectedProcess) refreshLogs();
            }
        } catch {}
    };
    ws.onclose = () => setTimeout(connectWS, 3000);
    ws.onerror = () => ws.close();
}

async function init() {
    try {
        allProcesses = await api('GET', '/processes');
        render(true);
    } catch {
        document.getElementById('process-grid').innerHTML = '<div class="empty-state"><h3>连接失败</h3><p>无法连接到守护进程</p></div>';
    }
    connectWS();
}
init();
