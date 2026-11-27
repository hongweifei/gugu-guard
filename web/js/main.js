// 入口：装配各层模块、切换视图与节点，并把界面动作落到接口调用。
// 依赖方向单向：core → ui / views / panels / forms → main。

import { $, busy, show } from './ui/dom.js';
import { api, onUnauthorized } from './core/api.js';
import { connectLive, disconnectLive } from './core/live.js';
import {
    setApiKey, setGroupFilter, setNode, setProcesses, setStatusFilter, setVersion,
    state, subscribe,
} from './core/state.js';
import { confirmAction, initConfirm } from './ui/dialog.js';
import { closeDrawer, initDrawer, openDrawer } from './ui/drawer.js';
import { toast } from './ui/toast.js';
import { initSidebar, loadNodes, notifyNodeUnreachable, setActiveView } from './views/sidebar.js';
import { focusSearch, initToolbar } from './views/toolbar.js';
import { initStats } from './views/stats.js';
import { initGroups } from './views/groups.js';
import { initProcesses } from './views/processes.js';
import { initOverview, overviewActivate } from './panels/overview.js';
import { initLogs, logsActivate } from './panels/logs.js';
import { metricsActivate } from './panels/metrics.js';
import { auditActivate, initAudit } from './panels/audit.js';
import { initTerminal, terminalActivate, terminalDeactivate } from './panels/terminal.js';
import { initUpdate, refreshUpdateBadge } from './panels/update.js';
import { initFsPicker } from './forms/fs.js';
import { initPrompt } from './ui/prompt.js';
import { initFiles, filesActivate, filesDeactivate, filesReset } from './views/files.js';
import { initProcessForm, openProcessForm } from './forms/process.js';

const VIEWS = {
    processes: 'view-processes',
    files: 'view-files',
    audit: 'view-audit',
    terminal: 'view-terminal',
};

/// 顶栏里只对进程视图有意义的控件。
const PROCESS_ONLY = ['search-box', 'btn-refresh', 'btn-add'];

let view = 'processes';
let booted = false;

function boot() {
    initConfirm();
    initPrompt();
    initFsPicker();
    initSidebar({ onNodeChange: switchNode, onNavigate: navigate });
    initToolbar({ onRefresh: refresh, onAdd: () => openProcessForm() });
    initStats();
    initGroups();
    initProcesses({
        onOpen: (name, tab) => openDrawer(name, tab),
        onAction: runAction,
        onEdit: (name) => openProcessForm(name),
        onRemove: removeProcess,
        onHealth: checkHealth,
        onFilterGroup: (group) => setGroupFilter(state.groupFilter === group ? null : group),
    });
    initDrawer({
        onTab: (tab, name) => activatePanels(name, tab),
        onClose: () => activatePanels(null, null),
        onAction: runAction,
    });
    initOverview({
        onEdit: (name) => openProcessForm(name),
        onRemove: removeProcess,
        onHealth: checkHealth,
    });
    initLogs();
    initFiles();
    initAudit();
    initTerminal();
    initUpdate({ onRestarted: afterUpdateRestart });
    initProcessForm({ onSaved: afterSave });

    onUnauthorized(showLogin);
    bindLogin();
    bindShortcuts();

    document.addEventListener('gugu:open-tab', (event) => {
        const { name, tab } = event.detail || {};
        if (name) openDrawer(name, tab || 'overview');
    });

    subscribe((reason) => {
        if (reason === 'reset' && booted) closeDrawer();
    });

    navigate('processes');
    start();
}

async function start() {
    await Promise.all([loadSelf(), loadNodes()]);
    await loadProcesses();
    booted = true;
    connectLive();
    refreshUpdateBadge();
}

/// 更新替换后守护进程是新进程：把版本、进程列表与节点表重新读一遍。
async function afterUpdateRestart() {
    await loadSelf();
    await loadProcesses();
    loadNodes();
}

async function loadSelf() {
    try {
        const info = await api('GET', '/node');
        setVersion(info?.version);
    } catch {
        // 版本号只是脚注，取不到不影响使用。
    }
}

async function loadProcesses() {
    try {
        const list = await api('GET', '/processes');
        setProcesses(list || []);
        showBanner('');
    } catch (error) {
        if (error.message === 'Unauthorized') return;
        setProcesses([]);
        showBanner(`${nodeLabel()} 读不到进程列表：${error.message}`);
        notifyNodeUnreachable(state.nodes.find((node) => node.id === state.node));
    }
}

/// 手动刷新只拉一次列表；常态由 WebSocket 每 2 秒推送。
async function refresh() {
    const button = $('btn-refresh');
    busy(button, true);
    await loadProcesses();
    busy(button, false);
}

async function switchNode(id) {
    if (!setNode(id)) return;
    disconnectLive();
    closeDrawer();
    filesReset();
    setProcesses([]);
    setStatusFilter('all');
    setGroupFilter(null);
    showBanner('');
    if (view !== 'processes') navigate('processes');
    await loadProcesses();
    connectLive();
}

function navigate(next) {
    if (!VIEWS[next]) return;
    if (next === 'terminal' && state.node !== 'local') {
        toast('终端只在本机可用，切回本机后再打开', 'info');
        return;
    }
    view = next;
    for (const [name, id] of Object.entries(VIEWS)) {
        show($(id), name === next);
    }
    for (const id of PROCESS_ONLY) {
        show($(id), next === 'processes');
    }
    if (next !== 'processes') closeDrawer();
    setActiveView(next);

    if (next === 'audit') auditActivate();
    if (next === 'files') {
        filesActivate();
    } else {
        filesDeactivate();
    }
    if (next === 'terminal') {
        terminalActivate();
    } else {
        terminalDeactivate();
    }
}

/// 抽屉三页只在所在标签激活时工作；`tab` 为空表示整页停用。
function activatePanels(name, tab) {
    const target = name && tab ? tab : null;
    overviewActivate(Boolean(target) && target === 'overview');
    logsActivate(target ? name : null, target === 'logs');
    metricsActivate(target ? name : null, target === 'metrics');
}

async function runAction(action, name, button) {
    if (!name) return;
    if (button && button.tagName === 'BUTTON') busy(button, true);
    try {
        const result = await api('POST', `/processes/${encodeURIComponent(name)}/${action}`);
        toast(result?.message || '操作已提交', 'success');
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '操作失败', 'error');
    } finally {
        if (button && button.tagName === 'BUTTON' && document.contains(button)) busy(button, false);
        loadProcesses();
    }
}

async function checkHealth(name) {
    try {
        const result = await api('POST', `/processes/${encodeURIComponent(name)}/health`);
        toast(result?.message || '健康检查已执行', 'success');
        loadProcesses();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '健康检查失败', 'error');
    }
}

async function removeProcess(name) {
    const ok = await confirmAction({
        title: '删除进程',
        text: `确认从配置中移除「${name}」？正在运行的进程会先被停止，日志文件保留在磁盘上。`,
        okText: '删除',
        danger: true,
    });
    if (!ok) return;
    try {
        const result = await api('DELETE', `/processes/${encodeURIComponent(name)}`);
        toast(result?.message || '进程已移除', 'success');
        if (state.selected === name) closeDrawer();
        await loadProcesses();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '删除失败', 'error');
    }
}

function afterSave(name) {
    loadProcesses();
    loadNodes();
    if (name) openDrawer(name, 'overview');
}

function nodeLabel() {
    const node = state.nodes.find((item) => item.id === state.node);
    return node?.name || (state.node === 'local' ? '本机' : state.node);
}

function showBanner(text) {
    const banner = $('node-banner');
    if (!text) {
        banner.hidden = true;
        return;
    }
    $('node-banner-text').textContent = text;
    banner.hidden = false;
}

function bindShortcuts() {
    document.addEventListener('keydown', (event) => {
        if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return;
        const tag = document.activeElement?.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || document.activeElement?.isContentEditable) return;
        if (view !== 'processes') return;
        event.preventDefault();
        focusSearch();
    });
}

// ── 登录 ────────────────────────────────────────────────

function bindLogin() {
    $('login-toggle').addEventListener('click', () => {
        const input = $('login-key');
        const masked = input.type === 'password';
        input.type = masked ? 'text' : 'password';
        $('login-toggle').textContent = masked ? '隐藏' : '显示';
    });
    $('login-submit').addEventListener('click', submitLogin);
    $('login-key').addEventListener('keydown', (event) => {
        if (event.key === 'Enter') submitLogin();
    });
}

let loginPending = false;

function showLogin() {
    if (loginPending) return;
    loginPending = true;
    const layer = $('login-layer');
    layer.classList.add('open');
    layer.setAttribute('aria-hidden', 'false');
    $('login-error').textContent = '';
    const input = $('login-key');
    input.value = '';
    input.focus();
}

function hideLogin() {
    loginPending = false;
    const layer = $('login-layer');
    layer.classList.remove('open');
    layer.setAttribute('aria-hidden', 'true');
}

async function submitLogin() {
    const key = $('login-key').value.trim();
    if (!key) {
        $('login-error').textContent = '请输入 API Key';
        return;
    }
    const button = $('login-submit');
    busy(button, true);
    setApiKey(key);
    try {
        const list = await api('GET', '/processes');
        setProcesses(list || []);
        hideLogin();
        toast('已登录', 'success');
        await Promise.all([loadSelf(), loadNodes()]);
        connectLive();
    } catch (error) {
        setApiKey('');
        $('login-error').textContent = error.message === 'Unauthorized' ? 'API Key 不正确' : (error.message || '认证失败');
    } finally {
        busy(button, false);
    }
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot, { once: true });
} else {
    boot();
}
