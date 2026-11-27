// 终端视图：xterm.js 渲染 + 一条真实 PTY 的 WebSocket。
// 进入视图才建会话，离开即断开；终端只在本机节点可用。

import { $ } from '../ui/dom.js';
import { API_BASE, api } from '../core/api.js';
import { state } from '../core/state.js';

const THEME = {
    background: '#111722',
    foreground: '#e6ecf6',
    cursor: '#4c8dff',
    cursorAccent: '#111722',
    selectionBackground: 'rgba(76,141,255,.32)',
    black: '#172033',
    red: '#f0655d',
    green: '#37c07a',
    yellow: '#e0a33e',
    blue: '#4c8dff',
    magenta: '#a78bfa',
    cyan: '#49b6e8',
    white: '#e6ecf6',
    brightBlack: '#61708a',
    brightRed: '#ff8a84',
    brightGreen: '#5fd99a',
    brightYellow: '#f0bb66',
    brightBlue: '#7fadff',
    brightMagenta: '#c4b0fd',
    brightCyan: '#7fd0f4',
    brightWhite: '#ffffff',
};

let info = null;
let term = null;
let fit = null;
let socket = null;
let ready = false;
let fitTimer = null;
let active = false;

export function initTerminal() {
    $('btn-term-reconnect').addEventListener('click', () => {
        if (!info?.enabled) return;
        connect();
        term?.focus();
    });

    window.addEventListener('resize', () => {
        if (!active) return;
        clearTimeout(fitTimer);
        fitTimer = setTimeout(fitTerminal, 80);
    });
}

async function loadInfo() {
    if (state.node !== 'local') {
        info = null;
        return;
    }
    try {
        info = await api('GET', '/terminal');
    } catch {
        info = null;
    }
}

/// 进入终端视图。
export async function terminalActivate() {
    active = true;
    if (state.node !== 'local') {
        showHint('终端只在本机可用', '当前查看的是从节点。切回本机后即可打开终端——PTY 会话只存在于运行守护进程的那台机器上。');
        return;
    }
    await loadInfo();
    if (!info?.enabled) {
        renderHint();
        return;
    }
    $('term-hint').hidden = true;
    $('term-body').hidden = false;
    $('btn-term-reconnect').disabled = false;
    ensureTerminal();
    requestAnimationFrame(() => {
        fitTerminal();
        connect();
        term?.focus();
    });
}

/// 离开终端视图：结束会话并释放 xterm 实例。
export function terminalDeactivate() {
    active = false;
    disconnect();
    if (term) {
        term.dispose();
        term = null;
        fit = null;
    }
    $('term-body').replaceChildren();
    setStatus('未连接', 'closed');
}

function renderHint() {
    if (state.node !== 'local') {
        showHint('终端只在本机可用', '当前查看的是从节点。切回本机后即可打开终端。');
        return;
    }
    if (!info) {
        showHint('终端配置读取失败', '确认守护进程在线后点击「重连」重试。', true);
        return;
    }
    if (!info.enabled) {
        showHint('终端未启用', '内置终端默认关闭。在 <code>gugu.toml</code> 的 <code>[daemon.web]</code> 下加入 <code>terminal = true</code>，然后重启守护进程。监听非回环地址时还必须配置 <code>daemon.api_key</code>，否则守护进程拒绝启动——终端等于把 shell 交给任何能打开面板的人。');
        return;
    }
    $('term-hint').hidden = true;
    $('term-body').hidden = false;
    $('btn-term-reconnect').disabled = false;
}

function showHint(title, body, retry = false) {
    const hint = $('term-hint');
    hint.hidden = false;
    hint.innerHTML = `<div class="term-hint-title">${title}</div><p>${body}</p>`;
    $('term-body').hidden = true;
    $('term-meta').textContent = '';
    $('btn-term-reconnect').disabled = !retry;
    setStatus('未启用', 'closed');
}

function ensureTerminal() {
    if (term) return;
    const host = $('term-body');
    host.replaceChildren();
    term = new Terminal({
        fontFamily: "'Cascadia Code', 'JetBrains Mono', ui-monospace, monospace",
        fontSize: 14,
        lineHeight: 1.2,
        cursorBlink: true,
        scrollback: 10000,
        theme: THEME,
        allowProposedApi: true,
    });
    fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(host);

    term.onData((data) => {
        if (ready && socket?.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: 'input', data }));
        }
    });
    term.onResize(({ cols, rows }) => {
        if (ready && socket?.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: 'resize', cols, rows }));
        }
    });
}

function fitTerminal() {
    if (!term || !fit) return;
    try {
        fit.fit();
    } catch {
        // 容器尚未完成布局，等下一次 resize。
    }
}

function connect() {
    disconnect();
    ensureTerminal();

    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const token = state.apiKey ? `?token=${encodeURIComponent(state.apiKey)}` : '';
    setStatus('连接中', 'connecting');
    ready = false;

    const ws = new WebSocket(`${proto}//${location.host}${API_BASE}/terminal/ws${token}`);
    socket = ws;

    ws.onopen = () => {
        setStatus('已连接', 'open');
        fitTerminal();
    };
    ws.onmessage = (event) => {
        let message;
        try {
            message = JSON.parse(event.data);
        } catch {
            return;
        }
        if (message.type === 'ready') {
            ready = true;
            setStatus('已连接', 'open');
            $('term-meta').textContent = `${message.shell} · ${message.cwd} · 空闲 ${Math.round(message.idle_timeout_secs / 60)} 分钟自动关闭`;
        } else if (message.type === 'data') {
            term?.write(message.data);
        } else if (message.type === 'exit') {
            ready = false;
            const code = message.code == null ? '' : `（退出码 ${message.code}）`;
            term?.write(`\r\n\x1b[33m[会话结束] ${message.reason || ''}${code}\x1b[0m\r\n`);
            setStatus('已结束', 'closed');
        } else if (message.type === 'error') {
            term?.write(`\r\n\x1b[31m[错误] ${message.message}\x1b[0m\r\n`);
            setStatus('启动失败', 'closed');
        }
    };
    ws.onerror = () => {
        setStatus('连接失败', 'closed');
        ready = false;
    };
    ws.onclose = () => {
        if (socket !== ws) return;
        socket = null;
        ready = false;
        if (term) term.write('\r\n\x1b[90m[连接已断开]\x1b[0m\r\n');
        setStatus('已断开', 'closed');
    };
}

function disconnect() {
    if (socket) {
        const ws = socket;
        socket = null;
        try {
            ws.close();
        } catch {
            // 已经关闭。
        }
    }
    ready = false;
}

function setStatus(text, kind) {
    const node = $('term-status');
    node.dataset.state = kind;
    $('term-status-text').textContent = text;
}
