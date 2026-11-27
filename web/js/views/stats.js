// 统计条：总量与状态分布可点击筛选，右侧两条是整机带宽读数。

import { $ } from '../ui/dom.js';
import { fmtRate, statusMeta } from '../core/format.js';
import { setStatusFilter, state, subscribe } from '../core/state.js';

export function initStats() {
    document.querySelectorAll('.stat[data-filter]').forEach((button) => {
        button.addEventListener('click', () => {
            const next = button.dataset.filter;
            setStatusFilter(state.statusFilter === next ? 'all' : next);
        });
    });

    subscribe((reason) => {
        if (reason === 'processes' || reason === 'filters' || reason === 'reset') renderStats();
        if (reason === 'traffic' || reason === 'processes' || reason === 'reset') renderTraffic();
    });

    renderStats();
    renderTraffic();
}

function renderStats() {
    const counts = { all: state.processes.length, running: 0, stopped: 0, failed: 0 };
    for (const process of state.processes) {
        const key = statusMeta(process).key;
        if (key === 'running') counts.running += 1;
        else if (key === 'failed') counts.failed += 1;
        else if (key === 'stopped') counts.stopped += 1;
    }

    $('stat-total').textContent = counts.all;
    $('stat-running').textContent = counts.running;
    $('stat-stopped').textContent = counts.stopped;
    $('stat-failed').textContent = counts.failed;

    document.querySelectorAll('.stat[data-filter]').forEach((button) => {
        button.setAttribute('aria-pressed', String(button.dataset.filter === state.statusFilter));
    });
}

function renderTraffic() {
    const traffic = state.traffic;
    $('stat-rx').textContent = traffic ? fmtRate(traffic.rx_bytes_per_sec) : '—';
    $('stat-tx').textContent = traffic ? fmtRate(traffic.tx_bytes_per_sec) : '—';
}
