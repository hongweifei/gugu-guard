// 事件接线与启动：确认对话框、日志工具栏、搜索/分组/卡片委托、登录、init。
// 本文件必须最后加载（内部直接调用前面文件定义的函数并启动应用）。

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

document.getElementById('group-bar').addEventListener('click', (e) => {
    const gact = e.target.closest('[data-gact]');
    if (gact) {
        e.stopPropagation();
        doGroupAction(gact.dataset.gact, gact.dataset.gname);
        return;
    }
    const chip = e.target.closest('[data-group]');
    if (chip) {
        const g = chip.dataset.group;
        groupFilter = g === '' ? null : (groupFilter === g ? null : g);
        render(true);
    }
});

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
        case 'filter-group': groupFilter = btn.dataset.group; render(true); break;
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
        connectWS();
    } catch {
        document.getElementById('process-grid').innerHTML = '<div class="empty-state"><h3>连接失败</h3><p>无法连接到守护进程</p></div>';
    }
}
init();
