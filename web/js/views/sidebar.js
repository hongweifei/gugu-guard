// 侧栏：品牌、节点切换、视图导航与连接状态。

import { $, clear, el } from '../ui/dom.js';
import { hubApi } from '../core/api.js';
import { setNodes, state, subscribe } from '../core/state.js';
import { toast } from '../ui/toast.js';

const VIEWS = {
    processes: { title: '进程', sub: '受管进程的状态、资源与日志' },
    files: { title: '文件', sub: '白名单目录内的浏览、预览与上传下载' },
    audit: { title: '操作审计', sub: '谁在何时对哪个进程做了什么，写操作才记录' },
    terminal: { title: '终端', sub: '本机 PTY 会话，关闭页面即结束' },
};

let hooks = { onNodeChange: () => {}, onNavigate: () => {} };
let activeView = 'processes';

export function initSidebar(config) {
    hooks = { ...hooks, ...config };

    document.querySelectorAll('.nav-item').forEach((button) => {
        button.addEventListener('click', () => {
            if (button.disabled) return;
            hooks.onNavigate(button.dataset.view);
        });
    });

    const button = $('node-button');
    button.addEventListener('click', () => {
        const menu = $('node-menu');
        const open = menu.hidden;
        menu.hidden = !open;
        button.setAttribute('aria-expanded', String(open));
    });

    document.addEventListener('click', (event) => {
        if (event.target.closest('.node-switch')) return;
        $('node-menu').hidden = true;
        $('node-button').setAttribute('aria-expanded', 'false');
    });

    subscribe((reason) => {
        if (reason === 'nodes' || reason === 'node' || reason === 'live') renderSidebar();
    });
    renderSidebar();
}

/// 节点表来自中心节点自身，与当前所选节点无关；取不到时保留本机一项。
export async function loadNodes() {
    try {
        const data = await hubApi('GET', '/nodes');
        setNodes(data?.nodes || []);
    } catch {
        // 认证失败由登录流程处理；网络抖动保留上一次的表。
    }
}

export function setActiveView(view) {
    activeView = view;
    document.querySelectorAll('.nav-item').forEach((button) => {
        const on = button.dataset.view === view;
        if (on) button.setAttribute('aria-current', 'page');
        else button.removeAttribute('aria-current');
    });
    const meta = VIEWS[view] || VIEWS.processes;
    $('view-title').textContent = meta.title;
    $('view-sub').textContent = meta.sub;
}

export function currentView() {
    return activeView;
}

function renderSidebar() {
    const current = state.nodes.find((node) => node.id === state.node);
    const local = state.nodes.find((node) => node.local);
    const name = current?.name || (state.node === 'local' ? local?.name || '本机' : state.node);
    $('node-name').textContent = name;

    const meta = $('node-meta');
    if (!current || current.local) {
        meta.textContent = current?.hostname || 'local';
    } else if (current.online) {
        meta.textContent = current.latency_ms != null ? `在线 · ${current.latency_ms} ms` : '在线';
    } else {
        meta.textContent = '离线';
    }

    const dot = $('node-dot');
    if (!current || current.local) dot.dataset.kind = 'local';
    else dot.dataset.kind = current.online ? 'online' : 'offline';

    renderNodeMenu();

    const terminalNav = $('nav-terminal');
    const blocked = state.node !== 'local';
    terminalNav.disabled = blocked;
    terminalNav.title = blocked ? '终端只在本机可用，切回本机后再打开' : '';

    renderConnection();
}

function renderNodeMenu() {
    const menu = $('node-menu');
    const nodes = state.nodes.length
        ? state.nodes
        : [{ id: 'local', name: '本机', local: true, online: true }];
    clear(menu);
    for (const node of nodes) {
        const option = el('button', {
            type: 'button',
            class: 'node-option',
            role: 'option',
            'aria-selected': String(node.id === state.node),
            onclick: () => {
                menu.hidden = true;
                $('node-button').setAttribute('aria-expanded', 'false');
                if (node.id === state.node) return;
                hooks.onNodeChange(node.id);
            },
        }, [
            el('span', {
                class: 'dot',
                'data-kind': node.local ? 'local' : (node.online ? 'online' : 'offline'),
            }),
            el('span', { class: 'node-option-text' }, [
                el('span', { class: 'node-option-name', text: node.local ? `${node.name}（本机）` : node.name }),
                el('span', {
                    class: 'node-option-meta',
                    text: node.local
                        ? (node.hostname || 'local')
                        : (node.online
                            ? `在线${node.latency_ms != null ? ` · ${node.latency_ms} ms` : ''}`
                            : (node.last_error || '离线')),
                }),
            ]),
        ]);
        menu.append(option);
    }
}

function renderConnection() {
    const conn = $('conn-state');
    conn.dataset.state = state.live;
    $('conn-text').textContent = state.live === 'open'
        ? '实时连接'
        : (state.live === 'connecting' ? '连接中' : '连接断开');
    $('build-info').textContent = state.version ? `v${state.version}` : '';
}

export function notifyNodeUnreachable(node) {
    toast(`节点 ${node?.name || state.node} 暂时读不到进程列表`, 'error');
}
