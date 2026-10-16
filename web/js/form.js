// 添加/编辑进程模态框、健康检查表单与文件系统浏览器。

function openModal(title, submit) {
    const backdrop = document.getElementById('modal-backdrop');
    if (!backdrop) return;
    document.getElementById('modal-title').textContent = title;
    document.getElementById('btn-submit-form').textContent = submit;
    backdrop.classList.add('open');
    updatePreview();
}

function closeModal() {
    const backdrop = document.getElementById('modal-backdrop');
    if (!backdrop) return;
    backdrop.classList.remove('open');
    document.getElementById('process-form').reset();
    document.getElementById('f-auto-start').checked = true;
    document.getElementById('f-auto-restart').checked = true;
    document.getElementById('f-group').value = '';
    document.getElementById('args-list').innerHTML = '';
    document.getElementById('env-list').innerHTML = '';
    document.getElementById('f-name').classList.remove('duplicate');
    document.getElementById('f-name-hint').className = 'field-hint';
    document.getElementById('f-name-hint').textContent = '';
    resetHealthCheck();
    editingName = null;
    editingBase = null;
}

window.addArgRow = function(value = '') {
    const div = document.createElement('div');
    div.className = 'dyn-row';
    div.innerHTML = `<input type="text" class="dyn-input arg-input" placeholder="--arg value" value="${esc(value)}"><button type="button" class="btn-icon btn-remove" onclick="this.parentElement.remove();updatePreview()" title="删除"><svg viewBox="0 0 20 20" fill="currentColor" width="14" height="14"><path fill-rule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 011.414 1.414L10 11.414l-4.293 4.293a1 1 0 010-1.414z"/></svg></button>`;
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
        fail_threshold: parseInt(document.getElementById('f-hc-fail').value) || 3,
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
    document.getElementById('f-hc-fail').value = hc.fail_threshold || 3;
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
    document.getElementById('f-hc-fail').value = 3;
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

// ── 文件系统浏览器 ─────────────────────────────────────────

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
    fsSelectedItem = null;
    fsActiveEl = null;
    fsSelectDir = false;
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

// ── 编辑回填与提交 ─────────────────────────────────────────

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
        // 记住完整原配置：提交时以它为底稿，表单没暴露的字段
        // (depends_on、max_log_size_mb 等) 才不会在保存后被重置。
        editingBase = cfg;
        f('f-dir').value = cfg.working_dir || '';
        f('f-stdout').value = cfg.stdout_log || '';
        f('f-stderr').value = cfg.stderr_log || '';
        f('f-group').value = cfg.group || '';
        f('f-max-restarts').value = cfg.max_restarts ?? 3;
        f('f-restart-delay').value = cfg.restart_delay_secs ?? 5;
        f('f-restart-reset').value = cfg.restart_reset_secs ?? 60;
        f('f-shell').checked = !!cfg.shell;
        f('f-stop-command').value = cfg.stop_command || '';
        f('f-stop-timeout').value = cfg.stop_timeout_secs ?? 10;
        if (cfg.env && typeof cfg.env === 'object') {
            Object.entries(cfg.env).forEach(([k, v]) => addEnvRow(k, v));
        }
        fillHealthCheck(cfg);
        openModal('编辑 — ' + name, '保存');
    } catch (e) {
        if (e.message === 'Unauthorized') {
            editingName = null;
            return;
        }
        editingBase = null;
        f('f-dir').value = '';
        f('f-stdout').value = '';
        f('f-stderr').value = '';
        f('f-max-restarts').value = 3;
        f('f-restart-delay').value = 5;
        f('f-restart-reset').value = 60;
        f('f-shell').checked = false;
        f('f-stop-timeout').value = 10;
        resetHealthCheck();
        openModal('编辑 — ' + name, '保存');
    }
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
        restart_reset_secs: parseInt(document.getElementById('f-restart-reset').value) || 60,
        shell: document.getElementById('f-shell').checked,
        stop_command: document.getElementById('f-stop-command').value.trim() || null,
        stop_timeout_secs: parseInt(document.getElementById('f-stop-timeout').value) || 10,
        health_check: hcData.health_check,
        unhealthy_restart: hcData.unhealthy_restart,
        stdout_log: document.getElementById('f-stdout').value || null,
        stderr_log: document.getElementById('f-stderr').value || null,
        group: document.getElementById('f-group').value.trim() || null,
        start_now: document.getElementById('f-auto-start').checked,
    };
    try {
        if (editingName) {
            if (newName !== editingName) body.new_name = newName;
            // PUT 是全量替换：以编辑时读回的完整配置为底稿，表单值覆盖其上，
            // 表单没有的字段 (depends_on、max_log_size_mb…) 原样保留。
            const payload = editingBase ? { ...editingBase, ...body } : body;
            const r = await api('PUT', `/processes/${encodeURIComponent(editingName)}`, payload);
            toast(r.message || '已更新', 'success');
        } else {
            const r = await api('POST', `/processes/${encodeURIComponent(newName)}`, body);
            toast(r.message || '已添加', 'success');
        }
        closeModal();
    } catch { toast('操作失败', 'error'); }
};
