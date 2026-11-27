// 版本与更新：查看当前/最新版本、检查、一键安装、调整自动更新策略。
// 安装会重启守护进程，页面在这期间会短暂掉线，重连成功后回读版本。

import { $, busy, show } from '../ui/dom.js';
import { api } from '../core/api.js';
import { fmtBytes, fmtDateTime } from '../core/format.js';
import { pushOverlay, popOverlay } from '../ui/overlay.js';
import { confirmAction } from '../ui/dialog.js';
import { toast } from '../ui/toast.js';
import { setVersion, subscribe } from '../core/state.js';

const LAYER = 'update-layer';
const POLL_INTERVAL = 1500;
const POLL_LIMIT = 80;

const AUTO_HINT = {
    off: '只在你打开本面板时检查，不自动下载。',
    check: '按间隔检查，发现新版本只在这里提示。',
    install: '按间隔检查并直接安装，替换后守护进程会自动重启。',
};

const PHASE_TEXT = {
    idle: '空闲',
    checking: '正在检查',
    downloading: '正在下载',
    verifying: '正在校验',
    applying: '正在替换',
    restarting: '正在重启',
    failed: '失败',
};

let hooks = { onRestarted: async () => {} };
let snapshot = null;
let pollTimer = null;
/// 安装开始时的版本，重连后用它判断版本是否真的换了。
let baselineVersion = '';

export function initUpdate(options = {}) {
    hooks = { onRestarted: options.onRestarted || (async () => {}) };

    $('update-entry').addEventListener('click', openPanel);
    $('upd-close').addEventListener('click', closePanel);
    $('upd-check').addEventListener('click', runCheck);
    $('upd-apply').addEventListener('click', applyUpdate);
    $('upd-auto').addEventListener('click', (event) => {
        const button = event.target.closest('.seg-btn');
        if (button) setAuto(button.dataset.auto);
    });

    subscribe((reason) => {
        if (reason === 'node' || reason === 'reset') {
            stopPolling();
            snapshot = null;
            setBadge(false);
        }
    });
}

/// 启动时静默读一次状态：有新版本就把侧栏的版本号点亮。
export async function refreshUpdateBadge() {
    try {
        const data = await api('GET', '/update');
        snapshot = data;
        setBadge(Boolean(data?.available));
    } catch {
        // 更新服务未启用或节点不可达：徽标保持熄灭，不影响主界面。
    }
}

async function openPanel() {
    pushOverlay(LAYER, closePanel);
    const layer = $(LAYER);
    layer.classList.add('open');
    layer.setAttribute('aria-hidden', 'false');
    render(snapshot);
    if (!snapshot || !snapshot.checked_at) {
        await runCheck();
    }
    $('upd-close').focus({ preventScroll: true });
}

function closePanel() {
    const layer = $(LAYER);
    layer.classList.remove('open');
    layer.setAttribute('aria-hidden', 'true');
    popOverlay(LAYER);
}

async function runCheck() {
    const button = $('upd-check');
    busy(button, true);
    renderPhase({ phase: 'checking', message: '正在检查更新' });
    try {
        const data = await api('POST', '/update/check');
        render(data);
        if (data?.available) {
            toast(`发现新版本 ${data.latest?.version || ''}`, 'info');
        } else {
            toast('已是最新版本', 'success');
        }
    } catch (error) {
        if (error.message !== 'Unauthorized') {
            renderPhase({ phase: 'failed', message: error.message });
            toast(error.message || '检查更新失败', 'error');
        }
    } finally {
        busy(button, false);
    }
}

async function setAuto(auto) {
    if (!auto || !snapshot || snapshot.config?.auto === auto) return;
    try {
        const data = await api('POST', '/update/settings', { auto });
        render(data);
        toast('自动更新设置已写入配置文件', 'success');
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '保存失败', 'error');
    }
}

async function applyUpdate() {
    const latest = snapshot?.latest?.version;
    if (!latest) {
        toast('先检查更新', 'info');
        return;
    }
    const ok = await confirmAction({
        title: '安装更新',
        text: `将下载并安装 ${latest}，替换当前程序后守护进程会重启。受管进程会先按各自的超时优雅停止，重启后再按配置拉起。`,
        okText: '安装',
    });
    if (!ok) return;

    const button = $('upd-apply');
    busy(button, true);
    try {
        await api('POST', '/update/apply', { version: latest });
        baselineVersion = snapshot?.current || '';
        renderPhase({ phase: 'downloading', message: `正在下载 ${latest}` });
        startPolling();
        toast('更新已开始，守护进程会在替换后自动重启', 'info', 6000);
    } catch (error) {
        busy(button, false);
        if (error.message !== 'Unauthorized') toast(error.message || '更新失败', 'error');
    }
}

/// 安装期间轮询状态；进程重启会让请求失败几次，恢复后对比版本。
function startPolling() {
    stopPolling();
    let attempts = 0;
    let offline = false;
    pollTimer = setInterval(async () => {
        attempts += 1;
        if (attempts > POLL_LIMIT) {
            stopPolling();
            renderPhase({ phase: 'failed', message: '等待新版本超时，请查看暂存目录的 apply.log' });
            return;
        }
        try {
            const data = await api('GET', '/update');
            if (offline && data.current && data.current !== baselineVersion) {
                stopPolling();
                setVersion(data.current);
                render(data);
                toast(`已更新到 ${data.current}`, 'success', 6000);
                await hooks.onRestarted();
                // 新进程的更新状态是空的，补一次检查，面板才回到「已是最新版本」。
                await runCheck();
                return;
            }
            offline = false;
            render(data);
            if (data.phase === 'idle' || data.phase === 'failed') {
                stopPolling();
            }
        } catch (error) {
            if (error.message === 'Unauthorized') return;
            offline = true;
            renderPhase({ phase: 'restarting', message: '守护进程正在重启，等待重新连接' });
        }
    }, POLL_INTERVAL);
}

function stopPolling() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = null;
}

// ── 渲染 ────────────────────────────────────────────────

function render(data) {
    if (!data) return;
    snapshot = data;

    $('upd-current').textContent = data.current ? `v${data.current}` : '—';
    $('upd-platform').textContent = data.platform || '';
    const latest = data.latest;
    $('upd-latest').textContent = latest?.version ? `v${latest.version}` : '—';
    $('upd-published').textContent = latest?.published_at
        ? `${fmtDateTime(latest.published_at)} 发布`
        : '';
    $('upd-notes').textContent = (latest?.notes || '').trim() || '这一版没有附带说明。';

    const status = $('upd-status');
    if (data.error) {
        status.textContent = data.error;
        status.className = 'upd-status is-bad';
    } else if (!latest) {
        status.textContent = '尚未检查更新';
        status.className = 'upd-status';
    } else if (data.available) {
        status.textContent = data.can_apply
            ? `发现新版本 ${latest.version}，可以一键更新`
            : `发现新版本 ${latest.version}，当前环境需手动下载`;
        status.className = 'upd-status is-warn';
    } else {
        const checked = data.checked_at ? `，上次检查 ${fmtDateTime(data.checked_at)}` : '';
        status.textContent = `已是最新版本${checked}`;
        status.className = 'upd-status is-ok';
    }

    renderAuto(data.config?.auto || 'off');
    renderPhase(data);

    // 可用性与「安装中」共同决定禁用状态：busy() 会把 disabled 写回 false，
    // 因此这里直接按两者计算，避免「已是最新」时按钮还能点。
    const apply = $('upd-apply');
    const busyPhase = isBusyPhase(data.phase);
    apply.disabled = !data.available || !data.can_apply || busyPhase;
    apply.classList.toggle('loading', busyPhase);

    $('upd-note').textContent = describeSource(data.config);
    setBadge(Boolean(data.available));
}

function renderPhase(data) {
    const phase = data.phase || 'idle';
    const busyPhase = isBusyPhase(phase);
    show($('upd-progress'), busyPhase || phase === 'failed');
    $('upd-bar').style.width = `${Math.round((data.progress || 0) * 100)}%`;
    $('upd-progress-text').textContent = progressText(data);
    if (busyPhase || phase === 'failed') {
        const status = $('upd-status');
        status.textContent = `${PHASE_TEXT[phase] || phase}：${data.message || ''}`.replace(/：$/, '');
        status.className = `upd-status ${phase === 'failed' ? 'is-bad' : ''}`.trim();
    }
    if (phase === 'failed') {
        show($('upd-progress'), false);
        $('upd-apply').classList.remove('loading');
    }
}

function progressText(data) {
    const percent = Math.round((data.progress || 0) * 100);
    if (data.total) {
        return `${percent}% · ${fmtBytes(data.downloaded || 0)} / ${fmtBytes(data.total)}`;
    }
    if (data.downloaded) return fmtBytes(data.downloaded);
    return PHASE_TEXT[data.phase || 'idle'] || '';
}

function renderAuto(auto) {
    for (const button of $('upd-auto').querySelectorAll('.seg-btn')) {
        button.setAttribute('aria-pressed', String(button.dataset.auto === auto));
    }
    $('upd-auto-hint').textContent = AUTO_HINT[auto] || AUTO_HINT.off;
}

function describeSource(config) {
    if (!config) return '';
    if (!config.enabled) return '更新已关闭（daemon.update.enabled = false）';
    if (config.provider === 'manifest') return `来源：自建清单 ${config.manifest_url || ''}`;
    return `来源：${config.provider} ${config.repo} · 每 ${config.check_hours} 小时自动检查`;
}

function setBadge(available) {
    show($('update-dot'), Boolean(available));
    $('update-entry').title = available ? '有新版本可用' : '检查更新';
}

function isBusyPhase(phase) {
    return ['checking', 'downloading', 'verifying', 'applying', 'restarting'].includes(phase);
}
