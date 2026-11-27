// 表单取值/赋值：把「读一个字段」的样板收在一处，提交逻辑只描述结构。

import { $ } from '../ui/dom.js';

export function text(id) {
    return ($(id)?.value ?? '').trim();
}

export function raw(id) {
    return $(id)?.value ?? '';
}

export function checked(id) {
    return !!$(id)?.checked;
}

export function number(id, fallback) {
    const value = Number.parseInt($(id)?.value ?? '', 10);
    return Number.isFinite(value) ? value : fallback;
}

export function setText(id, value) {
    const node = $(id);
    if (node) node.value = value ?? '';
}

export function setChecked(id, value) {
    const node = $(id);
    if (node) node.checked = !!value;
}

/// 文本字段「空即 null」：接口用 null 表示未配置。
export function nullable(id) {
    const value = text(id);
    return value || null;
}
