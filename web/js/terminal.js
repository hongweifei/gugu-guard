// WebUI 内置终端：xterm.js 渲染 + 一个真实 PTY 的 WebSocket。
// 终端默认关闭，由 GET /api/v1/terminal 决定按钮状态；关闭时不保留任何会话。

let terminalInfo = null;
let terminal = null;
let terminalFit = null;
let terminalSocket = null;
let terminalReady = false;
let terminalFitTimer = null;

const TERMINAL_THEME = {
    background: '#0c1018',
    foreground: '#e8ecf4',
    cursor: '#3b82f6',
    cursorAccent: '#0c1018',
    selectionBackground: 'rgba(59,130,246,.35)',
    black: '#1a2236',
    red: '#ef4444',
    green: '#22c55e',
    yellow: '#f59e0b',
    blue: '#3b82f6',
    magenta: '#a78bfa',
    cyan: '#38bdf8',
    white: '#e8ecf4',
    brightBlack: '#546580',
    brightRed: '#f87171',
    brightGreen: '#4ade80',
    brightYellow: '#fbbf24',
    brightBlue: '#60a5fa',
    brightMagenta: '#c4b5fd',
    brightCyan: '#7dd3fc',
    brightWhite: '#f8fafc',
};

async function initTerminal() {
    const button = document.getElementById('btn-terminal');
    try {
        const info = await api('GET', '/terminal');
        terminalInfo = info;
        button.hidden = false;
        if (info.enabled) {
            button.classList.remove('terminal-off');
            button.title = `打开终端（${info.shell}，最多 ${info.max_sessions} 个会话）`;
        } else {
            button.classList.add('terminal-off');
            button.title = '终端未启用：在 gugu.toml 的 [daemon.web] 设置 terminal = true';
        }
    } catch {
        // 认证失败或服务不可达时不展示入口，登录成功后 app.js 会再试一次。
        button.hidden = true;
    }
}

function openTerminal() {
    const overlay = document.getElementById('terminal-overlay');
    const hint = document.getElementById('terminal-hint');
    const body = document.getElementById('terminal-body');
    overlay.classList.add('open');

    if (!terminalInfo?.enabled) {
        body.hidden = true;
        hint.hidden = false;
        document.getElementById('btn-terminal-reconnect').hidden = true;
        hint.innerHTML = `
            <div class="terminal-hint-title">终端未启用</div>
            <p>内置终端默认关闭。在配置文件的 <code>[daemon.web]</code> 下加入 <code>terminal = true</code>，然后重启守护进程。</p>
            <p>监听非回环地址时还必须配置 <code>daemon.api_key</code>，否则守护进程会拒绝启动——终端等于把 shell 交给任何能打开面板的人。</p>`;
        setTerminalStatus('未启用', 'off');
        return;
    }

    body.hidden = false;
    hint.hidden = true;
    document.getElementById('btn-terminal-reconnect').hidden = false;
    ensureTerminal();
    requestAnimationFrame(() => {
        fitTerminal();
        connectTerminal();
        terminal.focus();
    });
}

function closeTerminal() {
    disconnectTerminal();
    document.getElementById('terminal-overlay').classList.remove('open');
    if (terminal) {
        terminal.dispose();
        terminal = null;
        terminalFit = null;
    }
    document.getElementById('terminal-body').innerHTML = '';
}

window.addEventListener('resize', () => {
    if (!document.getElementById('terminal-overlay').classList.contains('open')) return;
    clearTimeout(terminalFitTimer);
    terminalFitTimer = setTimeout(fitTerminal, 80);
});

function ensureTerminal() {
    if (terminal) return;
    const body = document.getElementById('terminal-body');
    body.innerHTML = '';
    terminal = new Terminal({
        fontFamily: "'JetBrains Mono', 'Cascadia Code', 'Fira Code', monospace",
        fontSize: 14,
        lineHeight: 1.2,
        cursorBlink: true,
        scrollback: 10000,
        theme: TERMINAL_THEME,
        allowProposedApi: true,
    });
    terminalFit = new FitAddon.FitAddon();
    terminal.loadAddon(terminalFit);
    terminal.open(body);
    terminal.onData((data) => {
        if (terminalReady && terminalSocket?.readyState === WebSocket.OPEN) {
            terminalSocket.send(JSON.stringify({ type: 'input', data }));
        }
    });
    terminal.onResize(({ cols, rows }) => {
        if (terminalReady && terminalSocket?.readyState === WebSocket.OPEN) {
            terminalSocket.send(JSON.stringify({ type: 'resize', cols, rows }));
        }
    });
}

function fitTerminal() {
    if (!terminal || !terminalFit) return;
    try { terminalFit.fit(); } catch { /* 容器尚未布局完成，下次 resize 再算 */ }
}

function connectTerminal() {
    disconnectTerminal();
    ensureTerminal();

    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const token = API_KEY ? `?token=${encodeURIComponent(API_KEY)}` : '';
    const url = `${proto}//${location.host}${API}/terminal/ws${token}`;
    setTerminalStatus('连接中…', 'connecting');
    terminalReady = false;

    const socket = new WebSocket(url);
    terminalSocket = socket;

    socket.onopen = () => {
        setTerminalStatus('已连接', 'on');
        fitTerminal();
    };
    socket.onmessage = (event) => {
        let message;
        try { message = JSON.parse(event.data); } catch { return; }
        if (message.type === 'ready') {
            terminalReady = true;
            setTerminalStatus('已连接', 'on');
            document.getElementById('terminal-meta').textContent =
                `${message.shell} · ${message.cwd} · 空闲 ${Math.round(message.idle_timeout_secs / 60)} 分钟自动关闭`;
        } else if (message.type === 'data') {
            terminal.write(message.data);
        } else if (message.type === 'exit') {
            terminalReady = false;
            const code = message.code == null ? '' : `（退出码 ${message.code}）`;
            terminal.write(`\r\n\x1b[33m[会话结束] ${message.reason || ''}${code}\x1b[0m\r\n`);
            setTerminalStatus('已结束', 'off');
        } else if (message.type === 'error') {
            terminal.write(`\r\n\x1b[31m[错误] ${message.message}\x1b[0m\r\n`);
            setTerminalStatus('启动失败', 'off');
        }
    };
    socket.onerror = () => {
        setTerminalStatus('连接失败', 'off');
        terminalReady = false;
    };
    socket.onclose = () => {
        if (terminalSocket !== socket) return;
        terminalSocket = null;
        terminalReady = false;
        if (terminal) terminal.write('\r\n\x1b[90m[连接已断开]\x1b[0m\r\n');
        setTerminalStatus('已断开', 'off');
    };
}

function disconnectTerminal() {
    if (terminalSocket) {
        const socket = terminalSocket;
        terminalSocket = null;
        try { socket.close(); } catch { /* 已关闭 */ }
    }
    terminalReady = false;
}

function setTerminalStatus(text, kind) {
    const el = document.getElementById('terminal-status');
    el.textContent = text;
    el.className = `terminal-status ${kind || ''}`;
}

document.getElementById('btn-terminal').onclick = openTerminal;
document.getElementById('btn-close-terminal').onclick = closeTerminal;
document.getElementById('btn-terminal-reconnect').onclick = () => {
    if (!terminalInfo?.enabled) return;
    connectTerminal();
    terminal?.focus();
};
