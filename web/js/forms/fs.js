// 文件浏览器：给「选择文件 / 选择目录」两个入口提供同一套浏览交互。
// 目录模式只列目录，文件模式目录用于导航、文件用于选中。

import { $, el } from '../ui/dom.js';
import { api } from '../core/api.js';
import { esc } from '../core/format.js';
import { popOverlay, pushOverlay } from '../ui/overlay.js';
import { toast } from '../ui/toast.js';

const LAYER = 'fs';

let mode = 'dir';
let current = null;
let parent = null;
let selected = null;
let onPick = null;
let seq = 0;

export function initFsPicker() {
    $('fs-close').addEventListener('click', closeFs);
    $('fs-cancel').addEventListener('click', closeFs);
    $('fs-confirm').addEventListener('click', confirmPick);
    $('fs-up').addEventListener('click', () => {
        if (parent) browse(parent);
    });
    $('fs-go').addEventListener('click', goToTyped);
    $('fs-path').addEventListener('keydown', (event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        goToTyped();
    });
    $('fs-list').addEventListener('keydown', (event) => {
        if (event.key === 'Enter') confirmPick();
    });
}

/// `openFsPicker({ mode, start, onSelect })`：mode 为 `dir` 或 `file`。
export function openFsPicker({ mode: kind = 'dir', start = '', onSelect } = {}) {
    mode = kind === 'file' ? 'file' : 'dir';
    onPick = onSelect || null;
    selected = null;
    current = null;
    parent = null;

    $('fs-title').textContent = mode === 'dir' ? '选择目录' : '选择文件';
    $('fs-confirm').textContent = mode === 'dir' ? '选择此目录' : '选择此文件';
    $('fs-list').replaceChildren();

    const layer = $('fs-layer');
    layer.classList.add('open');
    layer.setAttribute('aria-hidden', 'false');
    pushOverlay(LAYER, closeFs);
    browse(start);
}

function closeFs() {
    const layer = $('fs-layer');
    layer.classList.remove('open');
    layer.setAttribute('aria-hidden', 'true');
    popOverlay(LAYER);
    onPick = null;
    seq += 1;
}

function goToTyped() {
    const path = $('fs-path').value.trim();
    if (path) browse(path);
}

async function browse(path) {
    const ticket = ++seq;
    try {
        const data = await api('GET', `/fs/list?path=${encodeURIComponent(path || '')}`);
        if (ticket !== seq) return;
        current = data.path;
        parent = data.parent;
        selected = null;
        $('fs-path').value = current;
        renderEntries(data.entries || []);
    } catch (error) {
        if (ticket !== seq) return;
        if (error.message === 'Unauthorized') return;
        $('fs-list').innerHTML = `<p class="fs-empty">${esc(error.message)}</p>`;
    }
}

function renderEntries(entries) {
    const list = $('fs-list');
    list.replaceChildren();
    const visible = entries.filter((entry) => entry.is_dir || mode === 'file');
    if (!visible.length) {
        list.innerHTML = `<p class="fs-empty">${mode === 'dir' ? '这里没有子目录' : '这个目录是空的'}</p>`;
        return;
    }
    for (const entry of visible) {
        const item = el('div', {
            class: `fs-item ${entry.is_dir ? 'dir' : 'file'}`,
            text: entry.name,
            title: entry.path,
        });
        item.addEventListener('click', () => {
            if (entry.is_dir) {
                browse(entry.path);
                return;
            }
            select(item, entry.path);
        });
        item.addEventListener('dblclick', () => {
            if (!entry.is_dir) confirmPick();
        });
        list.append(item);
    }
    list.focus({ preventScroll: true });
}

function select(item, path) {
    selected = path;
    for (const node of $('fs-list').querySelectorAll('.fs-item')) {
        node.classList.toggle('active', node === item);
    }
}

function confirmPick() {
    if (mode === 'dir') {
        if (!current) return;
        finish(current);
        return;
    }
    if (!selected) {
        toast('先选中一个文件', 'info');
        return;
    }
    finish(selected);
}

function finish(path) {
    const callback = onPick;
    closeFs();
    if (callback) callback(path);
}
