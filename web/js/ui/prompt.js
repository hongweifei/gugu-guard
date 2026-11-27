// 单行输入对话框：新建目录、新建文件、重命名与「复制到」共用一套输入层。

import { $ } from './dom.js';
import { popOverlay, pushOverlay } from './overlay.js';

let resolver = null;

function finish(value) {
    const layer = $('prompt-layer');
    layer.classList.remove('open');
    layer.setAttribute('aria-hidden', 'true');
    popOverlay('prompt');
    const done = resolver;
    resolver = null;
    if (done) done(value);
}

export function initPrompt() {
    const layer = $('prompt-layer');
    $('prompt-cancel').addEventListener('click', () => finish(null));
    $('prompt-ok').addEventListener('click', () => submit());
    $('prompt-input').addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        submit();
    });
    layer.addEventListener('mousedown', (event) => {
        if (event.target === layer) finish(null);
    });
}

function submit() {
    const value = $('prompt-input').value.trim();
    if (!value) return;
    finish(value);
}

/// `promptText({ title, label, value, okText, hint })` → Promise<string | null>，取消时为 null。
export function promptText({
    title = '输入',
    label = '',
    value = '',
    okText = '确定',
    hint = '',
} = {}) {
    const layer = $('prompt-layer');
    if (!layer) return Promise.resolve(null);

    $('prompt-title').textContent = title;
    $('prompt-label').textContent = label;
    const input = $('prompt-input');
    input.value = value;
    $('prompt-ok').textContent = okText;
    const note = $('prompt-hint');
    note.textContent = hint;
    note.hidden = !hint;

    layer.classList.add('open');
    layer.setAttribute('aria-hidden', 'false');
    pushOverlay('prompt', () => finish(null));
    input.focus({ preventScroll: true });
    input.setSelectionRange(value.length, value.length);

    return new Promise((resolve) => {
        resolver = resolve;
    });
}
