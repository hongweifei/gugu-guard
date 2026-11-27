// 确认对话框：Promise 化的「取消 / 确认」，供删除等不可逆操作使用。

import { $ } from './dom.js';
import { popOverlay, pushOverlay } from './overlay.js';

let resolver = null;

function layer() {
    return $('confirm-layer');
}

function finish(result) {
    const el = layer();
    if (el) {
        el.classList.remove('open');
        el.setAttribute('aria-hidden', 'true');
    }
    popOverlay('confirm');
    const done = resolver;
    resolver = null;
    if (done) done(result);
}

export function initConfirm() {
    const el = layer();
    $('confirm-cancel').addEventListener('click', () => finish(false));
    $('confirm-ok').addEventListener('click', () => finish(true));
    el.addEventListener('mousedown', (event) => {
        if (event.target === el) finish(false);
    });
}

/// `confirmAction({ title, text, okText, danger })` → Promise<boolean>
export function confirmAction({ title = '确认操作', text = '', okText = '确认', danger = false } = {}) {
    const el = layer();
    if (!el) return Promise.resolve(false);
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    const ok = $('confirm-ok');
    ok.textContent = okText;
    ok.classList.toggle('btn-danger', danger);
    ok.classList.toggle('btn-primary', !danger);

    el.classList.add('open');
    el.setAttribute('aria-hidden', 'false');
    ok.focus({ preventScroll: true });
    pushOverlay('confirm', () => finish(false));

    return new Promise((resolve) => {
        resolver = resolve;
    });
}
