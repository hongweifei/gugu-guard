// 进程抽屉壳：开关、三个标签页、头部标识与操作。内容由 panels 各自渲染。

import { $ } from './dom.js';
import { popOverlay, pushOverlay } from './overlay.js';
import { esc, statusMeta } from '../core/format.js';
import { currentProcess, selectProcess, state, subscribe } from '../core/state.js';

let hooks = { onOpen: () => {}, onTab: () => {}, onClose: () => {}, onAction: () => {} };
let tab = 'overview';
let open = false;

const TABS = ['overview', 'logs', 'metrics'];

export function initDrawer(config) {
    hooks = { ...hooks, ...config };

    for (const name of TABS) {
        $(`panel-${name}`).hidden = name !== tab;
    }

    document.querySelectorAll('.tabs .tab').forEach((button) => {
        button.addEventListener('click', () => setTab(button.dataset.tab));
    });

    $('dr-close').addEventListener('click', closeDrawer);

    $('dr-start').addEventListener('click', () => hooks.onAction('start', state.selected));
    $('dr-stop').addEventListener('click', () => hooks.onAction('stop', state.selected));
    $('dr-restart').addEventListener('click', () => hooks.onAction('restart', state.selected));

    subscribe((reason) => {
        if (!open) return;
        if (reason === 'processes' || reason === 'selection' || reason === 'reset') renderHead();
    });
}

/// 头部标识跟随实时快照；进程被移除时说明状态并禁用按钮。
function renderHead() {
    const process = currentProcess();
    $('dr-name').textContent = process?.name || state.selected || '—';

    const dot = $('dr-dot');
    const badges = $('dr-badges');
    if (!process) {
        dot.dataset.state = 'stopped';
        badges.textContent = '进程已不在列表中';
        syncDrawerHead(null);
        return;
    }

    const status = statusMeta(process);
    dot.dataset.state = status.key;
    const chips = [`<span class="badge badge-${status.tone}">${esc(status.text)}</span>`];
    if (process.group) chips.push(`<span class="badge badge-group">${esc(process.group)}</span>`);
    if (process.has_health_check) {
        const tone = process.healthy === true ? 'ok' : (process.healthy === false ? 'bad' : 'idle');
        const label = process.healthy === true ? '健康' : (process.healthy === false ? '异常' : '待检');
        chips.push(`<span class="badge badge-${tone}">${esc(label)}</span>`);
    }
    badges.innerHTML = chips.join('');
    syncDrawerHead(process);
}

export function activeTab() {
    return tab;
}

export function setTab(name) {
    if (!TABS.includes(name)) return;
    tab = name;
    document.querySelectorAll('.tabs .tab').forEach((button) => {
        button.setAttribute('aria-selected', String(button.dataset.tab === name));
    });
    for (const key of TABS) {
        $(`panel-${key}`).hidden = key !== name;
    }
    hooks.onTab(name, state.selected);
}

export function openDrawer(name, targetTab = 'overview') {
    if (!name) return;
    const switched = state.selected !== name;
    selectProcess(name);
    const drawer = $('drawer');
    drawer.classList.add('open');
    drawer.setAttribute('aria-hidden', 'false');
    open = true;
    pushOverlay('drawer', closeDrawer);
    renderHead();
    if (switched) {
        setTab(targetTab);
        hooks.onOpen(name);
    } else if (targetTab !== tab) {
        setTab(targetTab);
    }
}

export function closeDrawer() {
    const drawer = $('drawer');
    if (!drawer.classList.contains('open')) return;
    drawer.classList.remove('open');
    drawer.setAttribute('aria-hidden', 'true');
    open = false;
    popOverlay('drawer');
    selectProcess(null);
    hooks.onClose();
}

/// 头部按钮的可用状态跟随当前进程状态。
export function syncDrawerHead(process) {
    const running = process ? statusMeta(process).key === 'running' : false;
    $('dr-start').disabled = !process || running;
    $('dr-stop').disabled = !process || !running;
    $('dr-restart').disabled = !process;
}
