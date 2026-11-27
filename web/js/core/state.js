// 共享状态：只在这里声明可变状态与订阅，
// 视图与面板通过 setter 修改、通过 subscribe 接收变更，不直接改字段。

import { statusMeta } from './format.js';

const LS_NODE = 'gugu_node';
const LS_KEY = 'gugu_api_key';

/// 节点标识要能安全当 URL 路径段：`.`、`..` 与含斜杠的值一律回退本机。
export function safeNodeId(id) {
    return id && id !== '.' && id !== '..' && !id.includes('/') ? id : 'local';
}

export const state = {
    /// 当前节点：`local` 为本机，其余为中心节点配置里的从节点标识。
    node: safeNodeId(localStorage.getItem(LS_NODE) || 'local'),
    nodes: [],
    apiKey: localStorage.getItem(LS_KEY) || '',
    processes: [],
    traffic: null,
    /// WS 连接状态：connecting / open / closed。
    live: 'connecting',
    query: '',
    /// 状态筛选：all / running / stopped / failed。
    statusFilter: 'all',
    groupFilter: null,
    /// 抽屉里正在查看的进程名。
    selected: null,
    version: '',
};

if (localStorage.getItem(LS_NODE) !== state.node) {
    localStorage.setItem(LS_NODE, state.node);
}

const listeners = new Set();

export function subscribe(fn) {
    listeners.add(fn);
    return () => listeners.delete(fn);
}

export function emit(reason) {
    for (const fn of listeners) fn(reason);
}

export function setNode(id) {
    const next = safeNodeId(id);
    if (next === state.node) return false;
    state.node = next;
    localStorage.setItem(LS_NODE, next);
    emit('node');
    return true;
}

export function setNodes(nodes) {
    state.nodes = Array.isArray(nodes) ? nodes : [];
    emit('nodes');
}

export function setApiKey(key) {
    state.apiKey = key || '';
    if (state.apiKey) localStorage.setItem(LS_KEY, state.apiKey);
    else localStorage.removeItem(LS_KEY);
}

export function setProcesses(list) {
    state.processes = Array.isArray(list) ? list : [];
    emit('processes');
}

export function setTraffic(traffic) {
    state.traffic = traffic || null;
    emit('traffic');
}

export function setLive(status) {
    state.live = status;
    emit('live');
}

export function setVersion(version) {
    state.version = version || '';
    emit('live');
}

export function setQuery(query) {
    state.query = (query || '').toLowerCase().trim();
    emit('filters');
}

export function setStatusFilter(filter) {
    state.statusFilter = filter || 'all';
    emit('filters');
}

export function setGroupFilter(group) {
    state.groupFilter = group || null;
    emit('filters');
}

export function selectProcess(name) {
    state.selected = name || null;
    emit('selection');
}

export function currentProcess() {
    return state.processes.find((p) => p.name === state.selected) || null;
}

/// 切换节点时清空视图状态，避免上一个节点的列表与筛选留在屏幕上。
export function resetView() {
    state.processes = [];
    state.traffic = null;
    state.query = '';
    state.statusFilter = 'all';
    state.groupFilter = null;
    state.selected = null;
    emit('reset');
}

/// 过滤后的进程列表：搜索词、状态、分组三个条件取交集。
export function visibleProcesses() {
    const q = state.query;
    return state.processes.filter((p) => {
        if (state.groupFilter && p.group !== state.groupFilter) return false;
        if (state.statusFilter !== 'all' && statusMeta(p).key !== state.statusFilter) {
            return false;
        }
        if (!q) return true;
        return (p.name || '').toLowerCase().includes(q)
            || (p.command || '').toLowerCase().includes(q)
            || (p.group || '').toLowerCase().includes(q);
    });
}
