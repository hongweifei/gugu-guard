// 添加/编辑进程模态框：动态参数与环境变量行、命令预览、重名检查、
// 编辑回填与表单提交。健康检查与网络设置见 form-health.js，
// 文件系统浏览器见 form-fs.js。

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
    resetNetworkSettings();
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
        f('f-log-format').value = cfg.log_format || 'auto';
        f('f-encoding').value = cfg.encoding || 'auto';
        f('f-cpu-max').value = cfg.cpu_max || '';
        f('f-memory-max').value = cfg.memory_max || '';
        f('f-pids-max').value = cfg.pids_max || '';
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
        fillNetworkSettings(cfg);
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
        f('f-log-format').value = 'auto';
        f('f-encoding').value = 'auto';
        f('f-cpu-max').value = '';
        f('f-memory-max').value = '';
        f('f-pids-max').value = '';
        f('f-max-restarts').value = 3;
        f('f-restart-delay').value = 5;
        f('f-restart-reset').value = 60;
        f('f-shell').checked = false;
        f('f-stop-timeout').value = 10;
        resetHealthCheck();
        resetNetworkSettings();
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
        log_format: document.getElementById('f-log-format').value || 'auto',
        encoding: document.getElementById('f-encoding').value || 'auto',
        cpu_max: document.getElementById('f-cpu-max').value.trim() || null,
        memory_max: document.getElementById('f-memory-max').value.trim() || null,
        pids_max: document.getElementById('f-pids-max').value.trim() || null,
        group: document.getElementById('f-group').value.trim() || null,
        start_now: document.getElementById('f-auto-start').checked,
        ...collectNetworkSettings(),
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
