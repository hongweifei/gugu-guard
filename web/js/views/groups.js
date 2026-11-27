// 分组条：按分组筛选，并在芯片上直接批量启停。
// 分组来自进程列表本身，没有独立配置。

import { $, clear, el } from '../ui/dom.js';
import { api } from '../core/api.js';
import { setGroupFilter, state, subscribe } from '../core/state.js';
import { toast } from '../ui/toast.js';

export function initGroups() {
    subscribe((reason) => {
        if (reason === 'processes' || reason === 'filters' || reason === 'reset') renderGroups();
    });
    renderGroups();
}

function groupCounts() {
    const map = new Map();
    for (const process of state.processes) {
        if (!process.group) continue;
        map.set(process.group, (map.get(process.group) || 0) + 1);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

function renderGroups() {
    const bar = $('groups');
    const groups = groupCounts();
    clear(bar);
    if (!groups.length) {
        bar.hidden = true;
        return;
    }
    bar.hidden = false;

    bar.append(el('button', {
        type: 'button',
        class: 'group-chip',
        'aria-pressed': String(!state.groupFilter),
        text: '全部',
        onclick: () => setGroupFilter(null),
    }));

    for (const [name, count] of groups) {
        const chip = el('div', {
            class: 'group-chip',
            'aria-pressed': String(state.groupFilter === name),
            role: 'button',
            tabindex: '0',
            onclick: (event) => {
                if (event.target.closest('.group-acts')) return;
                setGroupFilter(state.groupFilter === name ? null : name);
            },
            onkeydown: (event) => {
                if (event.key !== 'Enter' && event.key !== ' ') return;
                event.preventDefault();
                setGroupFilter(state.groupFilter === name ? null : name);
            },
        }, [
            el('span', { class: 'group-chip-name', text: name }),
            el('span', { class: 'group-chip-count', text: String(count) }),
            el('span', { class: 'group-acts' }, [
                actionButton('start', '▶', `启动分组 ${name}`, name),
                actionButton('stop', '■', `停止分组 ${name}`, name),
                actionButton('restart', '↻', `重启分组 ${name}`, name),
            ]),
        ]);
        bar.append(chip);
    }
}

function actionButton(act, glyph, title, group) {
    return el('button', {
        type: 'button',
        'data-act': act,
        title,
        text: glyph,
        onclick: (event) => {
            event.stopPropagation();
            runGroupAction(act, group);
        },
    });
}

async function runGroupAction(act, group) {
    try {
        const result = await api('POST', `/groups/${encodeURIComponent(group)}/${act}`);
        toast(result?.message || '操作完成', result?.failed > 0 ? 'error' : 'success');
    } catch (error) {
        toast(error.message || '分组操作失败', 'error');
    }
}
