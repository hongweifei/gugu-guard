// 进程表单：新建与编辑共用一套字段，提交时拼成 `CreateProcessRequest`。
// 字段分组交由 forms/ 下的子模块收集，这里只负责编排与提交。

import { $, busy, toggleClass } from '../ui/dom.js';
import { api } from '../core/api.js';
import { state } from '../core/state.js';
import { popOverlay, pushOverlay } from '../ui/overlay.js';
import { toast } from '../ui/toast.js';
import { addArgRow, addEnvRow, clearRows, collectArgs, collectEnv } from './dynamic.js';
import { checked, nullable, number, raw, setChecked, setText, text } from './fields.js';
import { collectHealth, fillHealth, healthyRestart, initHealth, resetHealth } from './health.js';
import { collectAlerts, fillAlerts, resetAlerts } from './alerts.js';
import { collectNetwork, fillNetwork, resetNetwork } from './network.js';
import { openFsPicker } from './fs.js';

const LAYER = 'form';

let onSaved = null;
let editing = null;
let initialized = false;

export function initProcessForm(hooks = {}) {
    onSaved = hooks.onSaved || null;
    if (initialized) return;
    initialized = true;

    initHealth();
    $('form-close').addEventListener('click', closeForm);
    $('form-cancel').addEventListener('click', closeForm);
    $('form-layer').addEventListener('mousedown', (event) => {
        if (event.target === $('form-layer')) closeForm();
    });
    $('pf-add-arg').addEventListener('click', () => addArgRow());
    $('pf-add-env').addEventListener('click', () => addEnvRow());
    $('pf-pick-cmd').addEventListener('click', () => pickFile('pf-command'));
    $('pf-pick-dir').addEventListener('click', () => pickDir('pf-dir'));
    $('pf-name').addEventListener('input', () => {
        checkName();
        refreshPreview();
    });
    $('pf-command').addEventListener('input', refreshPreview);
    $('pf-shell').addEventListener('change', refreshPreview);
    document.addEventListener('gugu:args-changed', refreshPreview);
    for (const chip of $('form-jump').querySelectorAll('[data-jump]')) {
        chip.addEventListener('click', () => jumpTo(chip.dataset.jump));
    }
    $('process-form').addEventListener('submit', submit);
}

/// 打开表单：`name` 为空即新建，否则加载该进程的完整配置。
export async function openProcessForm(name = null) {
    initProcessForm();
    resetForm();
    editing = name ? { name } : null;

    $('form-title').textContent = name ? `编辑 ${name}` : '添加进程';
    $('form-submit').textContent = name ? '保存' : '添加';
    $('form-note').textContent = '';

    const layer = $('form-layer');
    layer.classList.add('open');
    layer.setAttribute('aria-hidden', 'false');
    pushOverlay(LAYER, closeForm);

    if (!name) {
        setText('pf-name', suggestName());
        $('pf-name').focus();
        $('pf-name').select();
        return;
    }

    busy($('form-submit'), true);
    try {
        const config = await api('GET', `/processes/${encodeURIComponent(name)}/config`);
        fillForm(name, config || {});
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '读取进程配置失败', 'error');
        closeForm();
        return;
    } finally {
        busy($('form-submit'), false);
    }
}

function closeForm() {
    const layer = $('form-layer');
    layer.classList.remove('open');
    layer.setAttribute('aria-hidden', 'true');
    popOverlay(LAYER);
    editing = null;
}

function jumpTo(sectionId) {
    const section = $(sectionId);
    if (!section) return;
    section.scrollIntoView({ behavior: 'smooth', block: 'start' });
}

function pickFile(targetId) {
    openFsPicker({
        mode: 'file',
        start: text('pf-dir') || text(targetId),
        onSelect: (path) => setText(targetId, path),
    });
}

function pickDir(targetId) {
    openFsPicker({
        mode: 'dir',
        start: text(targetId),
        onSelect: (path) => setText(targetId, path),
    });
}

function resetForm() {
    $('process-form').reset();
    clearRows();
    resetHealth();
    resetNetwork();
    resetAlerts();
    $('pf-name-hint').textContent = '';
    toggleClass($('pf-name-hint'), 'is-bad', false);
    refreshPreview();
}

function fillForm(name, config) {
    setText('pf-name', name);
    setText('pf-group', config.group || '');
    setText('pf-command', config.command || '');
    clearRows();
    (config.args || []).forEach((arg) => addArgRow(arg));
    Object.entries(config.env || {}).forEach(([key, value]) => addEnvRow(key, value));
    setText('pf-dir', config.working_dir || '');
    setText('pf-cpu-max', config.cpu_max || '');
    setText('pf-memory-max', config.memory_max || '');
    setText('pf-pids-max', config.pids_max || '');
    setText('pf-stdout', config.stdout_log || '');
    setText('pf-stderr', config.stderr_log || '');
    setText('pf-log-format', config.log_format || 'auto');
    setText('pf-encoding', config.encoding || 'auto');
    setChecked('pf-auto-start', config.auto_start !== false);
    setChecked('pf-auto-restart', config.auto_restart !== false);
    setChecked('pf-shell', !!config.shell);
    setText('pf-max-restarts', config.max_restarts ?? 3);
    setText('pf-restart-delay', config.restart_delay_secs ?? 5);
    setText('pf-restart-reset', config.restart_reset_secs ?? 60);
    setText('pf-stop-timeout', config.stop_timeout_secs ?? 10);
    setText('pf-stop-command', config.stop_command || '');
    fillHealth(config.health_check);
    setChecked('pf-unhealthy-restart', !!config.unhealthy_restart);
    fillNetwork(config);
    fillAlerts(config.log_alert);
    refreshPreview();
}

/// 新建时给一个不重名的占位名称，用户直接改也方便。
function suggestName() {
    const used = new Set(state.processes.map((p) => p.name));
    if (!used.has('new-process')) return 'new-process';
    for (let i = 2; i < 100; i += 1) {
        const candidate = `new-process-${i}`;
        if (!used.has(candidate)) return candidate;
    }
    return 'new-process';
}

function checkName() {
    const hint = $('pf-name-hint');
    const name = text('pf-name');
    if (!name) {
        hint.textContent = '';
        toggleClass(hint, 'is-bad', false);
        return true;
    }
    const original = editing?.name;
    const taken = state.processes.some((p) => p.name === name && p.name !== original);
    hint.textContent = taken ? '该名称已被占用' : '';
    toggleClass(hint, 'is-bad', taken);
    return !taken;
}

function refreshPreview() {
    const command = text('pf-command');
    const args = collectArgs();
    const shell = checked('pf-shell');
    const line = [command, ...args].filter(Boolean).join(' ');
    const node = $('pf-preview');
    if (!line) {
        node.textContent = '—';
        return;
    }
    node.textContent = shell ? `${shellPrefix()} ${line}` : line;
}

function shellPrefix() {
    const agent = navigator.userAgent;
    return agent.includes('Windows') ? 'cmd /C' : 'sh -c';
}

/// 编辑时以原配置为底稿：表单没有暴露的字段（依赖、内存阈值）必须原样带回，
/// 否则 PUT 整体替换会把它们清空。
async function submit(event) {
    event.preventDefault();
    if (!checkName()) {
        $('pf-name').focus();
        return;
    }
    const name = text('pf-name');
    if (!name) {
        toast('进程名称不能为空', 'error');
        $('pf-name').focus();
        return;
    }
    if (!text('pf-command')) {
        toast('执行命令不能为空', 'error');
        $('pf-command').focus();
        return;
    }

    const button = $('form-submit');
    busy(button, true);
    try {
        if (editing) {
            const base = await api('GET', `/processes/${encodeURIComponent(editing.name)}/config`).catch(() => null);
            const body = buildBody(base || {});
            body.new_name = name === editing.name ? null : name;
            await api('PUT', `/processes/${encodeURIComponent(editing.name)}`, body);
            toast('进程已更新', 'ok');
        } else {
            const body = buildBody({});
            body.start_now = body.auto_start;
            await api('POST', `/processes/${encodeURIComponent(name)}`, body);
            toast('进程已添加', 'ok');
        }
        closeForm();
        if (onSaved) onSaved(name);
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '保存失败', 'error');
    } finally {
        busy(button, false);
    }
}

function buildBody(base) {
    return {
        command: text('pf-command'),
        args: collectArgs(),
        working_dir: nullable('pf-dir'),
        env: collectEnv(),
        auto_start: checked('pf-auto-start'),
        auto_restart: checked('pf-auto-restart'),
        max_restarts: number('pf-max-restarts', 3),
        restart_delay_secs: number('pf-restart-delay', 5),
        restart_reset_secs: number('pf-restart-reset', 60),
        shell: checked('pf-shell'),
        stop_command: nullable('pf-stop-command'),
        stop_timeout_secs: number('pf-stop-timeout', 10),
        health_check: collectHealth(),
        unhealthy_restart: healthyRestart(),
        group: nullable('pf-group'),
        depends_on: base.depends_on || [],
        max_memory_mb: base.max_memory_mb ?? null,
        cpu_max: nullable('pf-cpu-max'),
        memory_max: nullable('pf-memory-max'),
        pids_max: nullable('pf-pids-max'),
        max_log_size_mb: base.max_log_size_mb ?? null,
        stdout_log: nullable('pf-stdout'),
        stderr_log: nullable('pf-stderr'),
        log_format: raw('pf-log-format'),
        encoding: raw('pf-encoding'),
        log_alert: collectAlerts(),
        ...collectNetwork(),
        start_now: false,
        new_name: null,
    };
}
