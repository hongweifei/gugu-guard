// 顶栏：搜索、刷新、添加。搜索只改状态，过滤由列表自己算。

import { $ } from '../ui/dom.js';
import { setQuery, state } from '../core/state.js';

export function initToolbar({ onRefresh, onAdd }) {
    const input = $('search-input');
    input.value = state.query;
    input.addEventListener('input', () => setQuery(input.value));

    $('btn-refresh').addEventListener('click', onRefresh);
    $('btn-add').addEventListener('click', onAdd);
}

export function focusSearch() {
    const input = $('search-input');
    input.focus();
    input.select();
}
