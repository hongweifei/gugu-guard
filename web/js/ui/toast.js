// 轻量提示条：右下角飘出，3 秒后自动收走。并发提示按到达顺序堆叠。

import { el } from './dom.js';

let host = null;

function ensureHost() {
    if (!host) host = document.getElementById('toast-host');
    return host;
}

export function toast(message, type = 'info', timeout = 3200) {
    const box = ensureHost();
    if (!box) return;
    const node = el('div', { class: `toast ${type}` }, [
        el('span', { text: message }),
    ]);
    box.append(node);
    setTimeout(() => {
        node.classList.add('leaving');
        setTimeout(() => node.remove(), 220);
    }, timeout);
}
