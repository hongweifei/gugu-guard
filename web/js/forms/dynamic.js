// 动态行：命令参数与环境变量。行的增删与收集都在这里，表单只调这几个函数。

import { $, el } from '../ui/dom.js';
import { esc } from '../core/format.js';

export function addArgRow(value = '') {
    const row = el('div', { class: 'row-item' });
    row.innerHTML = `<input type="text" class="arg-input" placeholder="--flag value" value="${esc(value)}" autocomplete="off" spellcheck="false">`;
    row.append(removeButton(() => row.remove()));
    $('pf-args').append(row);
    const input = row.querySelector('input');
    input.addEventListener('input', () => {
        document.dispatchEvent(new CustomEvent('gugu:args-changed'));
    });
    if (!value) input.focus();
}

export function addEnvRow(key = '', value = '') {
    const row = el('div', { class: 'row-item' });
    row.innerHTML = `<input type="text" class="env-key" placeholder="KEY" value="${esc(key)}" autocomplete="off" spellcheck="false">`
        + '<span class="row-sep">=</span>'
        + `<input type="text" class="env-value" placeholder="value" value="${esc(value)}" autocomplete="off" spellcheck="false">`;
    row.append(removeButton(() => row.remove()));
    $('pf-env').append(row);
    if (!key) row.querySelector('input').focus();
}

export function clearRows() {
    $('pf-args').replaceChildren();
    $('pf-env').replaceChildren();
}

export function collectArgs() {
    return [...document.querySelectorAll('#pf-args .arg-input')]
        .map((input) => input.value.trim())
        .filter(Boolean);
}

export function collectEnv() {
    const env = {};
    for (const row of document.querySelectorAll('#pf-env .row-item')) {
        const key = row.querySelector('.env-key').value.trim();
        const value = row.querySelector('.env-value').value.trim();
        if (key) env[key] = value;
    }
    return env;
}

function removeButton(onClick) {
    return el('button', {
        type: 'button',
        class: 'row-remove',
        title: '删除这一行',
        text: '×',
        onclick: onClick,
    });
}
