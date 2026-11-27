// 进程表格：行渲染、增量更新、行内操作与「更多」菜单。
// 表格只负责展示与派发意图，具体动作由 main 注入的回调执行。

import { $, el } from '../ui/dom.js';
import { icon } from '../ui/icons.js';
import { commandLine, esc, fmtBytes, fmtClock, fmtDuration, statusMeta } from '../core/format.js';
import { state, subscribe, visibleProcesses } from '../core/state.js';

let hooks = {
    onOpen: () => {},
    onAction: () => {},
    onEdit: () => {},
    onRemove: () => {},
    onHealth: () => {},
    onFilterGroup: () => {},
};

let rowKeys = '';
let floatingMenu = null;

export function initProcesses(config) {
    hooks = { ...hooks, ...config };

    const body = $('ptable-body');
    body.addEventListener('click', onTableClick);

    // 菜单是 fixed 定位：任何滚动都让它失去参照，直接收起。
    window.addEventListener('scroll', closeMenu, true);
    document.addEventListener('click', (event) => {
        if (!event.target.closest('.menu') && !event.target.closest('[data-act="menu"]')) closeMenu();
    });
    document.addEventListener('keydown', (event) => {
        if (event.key === 'Escape' && floatingMenu) {
            event.stopPropagation();
            closeMenu();
        }
    }, true);

    subscribe((reason) => {
        if (['processes', 'filters', 'selection', 'reset'].includes(reason)) render();
    });
    render();
}

function render() {
    const list = visibleProcesses();
    const keys = list.map((process) => process.name).join('\u0000');

    if (keys !== rowKeys) {
        closeMenu();
        $('ptable-body').innerHTML = list.map(rowHtml).join('');
        rowKeys = keys;
    } else {
        for (const process of list) patchRow(process);
    }

    renderEmpty(list);
    syncSelection();
}

function renderEmpty(list) {
    const empty = $('table-empty');
    const table = $('ptable');
    if (list.length) {
        empty.hidden = true;
        table.hidden = false;
        return;
    }
    table.hidden = true;
    empty.hidden = false;

    const searching = state.query || state.statusFilter !== 'all' || state.groupFilter;
    const title = searching ? '没有匹配的进程' : '暂无受管进程';
    const hint = searching
        ? '调整搜索词或清除筛选条件'
        : '点击右上角「添加进程」把要守护的服务登记进来';
    empty.innerHTML = `
        <svg viewBox="0 0 80 80" fill="none" stroke="currentColor" stroke-width="1.6">
            ${searching
                ? '<circle cx="36" cy="36" r="18"/><line x1="49" y1="49" x2="66" y2="66" stroke-linecap="round" stroke-width="4"/>'
                : '<rect x="10" y="22" width="60" height="38" rx="6"/><circle cx="30" cy="41" r="5"/><circle cx="50" cy="41" r="5"/>'}
        </svg>
        <h3>${esc(title)}</h3>
        <p>${esc(hint)}</p>`;
}

function rowHtml(process) {
    const status = statusMeta(process);
    return `<tr data-name="${esc(process.name)}">
        <td class="td-name">
            <div class="name-line">
                <span class="dot" data-state="${status.key}"></span>
                <span class="name" data-act="open" title="查看详情">${esc(process.name)}</span>
                ${status.key === 'running' ? '' : `<span class="badge badge-${status.tone}" data-badge="status">${esc(status.text)}</span>`}
            </div>
            <div class="name-under">
                ${process.group ? `<span class="badge badge-group" data-act="group" title="按分组筛选">${esc(process.group)}</span>` : ''}
                <span class="cmd" data-field="cmd" title="${esc(commandLine(process))}">${esc(commandLine(process))}</span>
            </div>
        </td>
        <td class="td-mono td-pid" data-field="pid">${process.pid ?? '—'}</td>
        <td class="td-mono td-uptime" data-field="uptime">${fmtDuration(process.uptime_secs)}</td>
        <td class="td-mono td-usage" data-field="usage">${usageLabel(process)}</td>
        <td class="td-mono td-ports" data-field="ports">${portsLabel(process)}</td>
        <td class="td-mono td-exit" data-field="exit">${exitLabel(process)}</td>
        <td class="td-health" data-field="health">${healthHtml(process)}</td>
        <td class="td-actions">
            <div class="row-actions">
                <button type="button" class="btn" data-act="start" ${status.key === 'running' ? 'disabled' : ''}>启动</button>
                <button type="button" class="btn" data-act="stop" ${status.key === 'running' ? '' : 'disabled'}>停止</button>
                <button type="button" class="btn-icon" data-act="logs" title="查看日志" aria-label="查看日志">${icon('logs')}</button>
                <button type="button" class="btn-icon" data-act="menu" title="更多操作" aria-label="更多操作">${icon('more')}</button>
            </div>
        </td>
    </tr>`;
}

function patchRow(process) {
    const row = $('ptable-body').querySelector(`tr[data-name="${CSS.escape(process.name)}"]`);
    if (!row) return;
    const status = statusMeta(process);

    row.querySelector('.dot').dataset.state = status.key;

    const badge = row.querySelector('[data-badge="status"]');
    if (status.key === 'running') {
        if (badge) badge.remove();
    } else if (badge) {
        badge.className = `badge badge-${status.tone}`;
        badge.textContent = status.text;
    } else {
        row.querySelector('.name-line').append(
            el('span', { class: `badge badge-${status.tone}`, dataset: { badge: 'status' }, text: status.text }),
        );
    }

    setField(row, 'cmd', commandLine(process), commandLine(process));
    setField(row, 'pid', process.pid ?? '—');
    setField(row, 'uptime', fmtDuration(process.uptime_secs));
    setField(row, 'usage', usageLabel(process));
    setField(row, 'ports', portsLabel(process));
    setField(row, 'exit', exitLabel(process));

    const health = row.querySelector('[data-field="health"]');
    if (health) health.innerHTML = healthHtml(process);

    const start = row.querySelector('[data-act="start"]');
    const stop = row.querySelector('[data-act="stop"]');
    if (start) start.disabled = status.key === 'running';
    if (stop) stop.disabled = status.key !== 'running';
}

function setField(row, name, text, title) {
    const node = row.querySelector(`[data-field="${name}"]`);
    if (!node) return;
    node.textContent = text;
    if (title !== undefined) node.title = title;
}

function usageLabel(process) {
    const usage = process.usage;
    if (!usage) return '—';
    return `${fmtBytes(usage.memory_bytes)} · ${usage.cpu_percent.toFixed(1)}%`;
}

function portsLabel(process) {
    const ports = process.listen_ports || [];
    if (!ports.length) return '—';
    return `${ports.join(' · ')} · ${process.connections || 0} 连接`;
}

function exitLabel(process) {
    const exits = process.recent_exits || [];
    if (!exits.length) return '—';
    const last = exits[exits.length - 1];
    const code = last.code == null ? '未知' : last.code;
    return `${code} · ${fmtClock(last.at)}${last.auto_restarted ? ' ↻' : ''}`;
}

function healthHtml(process) {
    if (!process.has_health_check) return '<span class="td-muted">—</span>';
    if (process.healthy === true) return '<span class="badge badge-ok">健康</span>';
    if (process.healthy === false) return '<span class="badge badge-bad">异常</span>';
    return '<span class="badge badge-idle">待检</span>';
}

function syncSelection() {
    $('ptable-body').querySelectorAll('tr').forEach((row) => {
        row.classList.toggle('selected', row.dataset.name === state.selected);
    });
}

function onTableClick(event) {
    const row = event.target.closest('tr[data-name]');
    if (!row) return;
    const name = row.dataset.name;
    const target = event.target.closest('[data-act]');

    if (!target) {
        hooks.onOpen(name, 'overview');
        return;
    }

    switch (target.dataset.act) {
        case 'open':
            hooks.onOpen(name, 'overview');
            break;
        case 'logs':
            hooks.onOpen(name, 'logs');
            break;
        case 'group':
            hooks.onFilterGroup(target.textContent);
            break;
        case 'start':
        case 'stop':
        case 'restart':
            hooks.onAction(target.dataset.act, name, target);
            break;
        case 'menu':
            event.stopPropagation();
            openMenu(target, name);
            break;
        default:
            break;
    }
}

function openMenu(anchor, name) {
    closeMenu();
    const process = state.processes.find((item) => item.name === name);
    if (!process) return;

    const items = [
        { label: '编辑配置', icon: 'edit', act: () => hooks.onEdit(name) },
        { label: '查看指标', icon: 'chart', act: () => hooks.onOpen(name, 'metrics') },
        { label: '查看日志', icon: 'logs', act: () => hooks.onOpen(name, 'logs') },
    ];
    if (process.has_health_check) {
        items.push({ label: '立即检查', icon: 'health', act: () => hooks.onHealth(name) });
    }

    const menu = el('div', { class: 'menu menu-floating' },
        items.map((item) => el('button', {
            type: 'button',
            onclick: () => {
                closeMenu();
                item.act();
            },
        }, [
            el('span', { innerHTML: icon(item.icon, 14) }),
            el('span', { text: item.label }),
        ])),
    );

    menu.append(el('div', { class: 'menu-sep' }));
    menu.append(el('button', {
        type: 'button',
        class: 'danger',
        onclick: () => {
            closeMenu();
            hooks.onRemove(name);
        },
    }, [
        el('span', { innerHTML: icon('trash', 14) }),
        el('span', { text: '删除进程' }),
    ]));

    document.body.append(menu);
    const rect = anchor.getBoundingClientRect();
    const width = menu.offsetWidth || 140;
    const height = menu.offsetHeight || 160;
    const left = Math.min(rect.right - width, window.innerWidth - width - 8);
    const top = rect.bottom + height + 8 > window.innerHeight ? rect.top - height - 4 : rect.bottom + 4;
    menu.style.left = `${Math.max(8, left)}px`;
    menu.style.top = `${Math.max(8, top)}px`;
    floatingMenu = menu;
    anchor.setAttribute('aria-expanded', 'true');
}

function closeMenu() {
    if (!floatingMenu) return;
    floatingMenu.remove();
    floatingMenu = null;
    document.querySelectorAll('[data-act="menu"][aria-expanded]').forEach((node) => node.removeAttribute('aria-expanded'));
}
