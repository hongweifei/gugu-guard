// 全局状态与基础工具。所有跨模块共享的可变状态只在这里声明一次；
// 其余文件只读写，不再重复声明。加载顺序见 index.html 底部的 <script> 链。

const API = '/api/v1';
/// 当前查看的节点：`local` 是本机，其余为中心节点配置里的从节点标识。
/// 切换入口在 nodes.js，REST 与实时推送都跟随这个值。
let currentNode = localStorage.getItem('gugu_node') || 'local';
let ws = null;
let selectedProcess = null;
let editingName = null;
let editingBase = null;
let allProcesses = [];
let searchQuery = '';
let groupFilter = null;
let API_KEY = localStorage.getItem('gugu_api_key') || '';
let logEntries = [];
let logSearchQuery = '';
let logStreamFilter = 'all';
/// 最低级别过滤：all / debug / info / warn / error / fatal。
let logLevelFilter = 'all';
/// 日志面板数据来源：live（实时环形缓冲）或 file（落盘历史文件检索结果）。
let logMode = 'live';
let logLineHeight = 22;
let logVisibleCount = 0;
let logScrollTop = 0;
/// 最近一次整机带宽读数，由 WS status 消息的 system 字段更新。
let systemTraffic = null;

/// 把接口路径拼上当前节点前缀；本机不加前缀，从节点走中心节点的代理路径。
function apiPath(path) {
    return currentNode === 'local' ? path : `/n/${encodeURIComponent(currentNode)}${path}`;
}

async function request(method, path, body) {
    const opts = { method };
    if (body) { opts.headers = { 'Content-Type': 'application/json' }; opts.body = JSON.stringify(body); }
    if (API_KEY) {
        opts.headers = opts.headers || {};
        opts.headers['Authorization'] = `Bearer ${API_KEY}`;
    }
    const resp = await fetch(`${API}${path}`, opts);
    if (resp.status === 401) {
        showLogin();
        throw new Error('Unauthorized');
    }
    return resp.json();
}

/// 业务接口：自动跟随当前节点。
async function api(method, path, body) {
    return request(method, apiPath(path), body);
}

/// 只发给中心节点自身的接口（节点表），不跟随所选节点。
async function hubApi(method, path, body) {
    return request(method, path, body);
}

function esc(t) { return t.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;'); }
function fmtTime(s) { if (s == null) return '-'; return `${String(Math.floor(s/3600)).padStart(2,'0')}:${String(Math.floor((s%3600)/60)).padStart(2,'0')}:${String(s%60).padStart(2,'0')}`; }

function statusOf(p) {
    const isObj = typeof p.status === 'object' && p.status !== null;
    const raw = isObj ? Object.keys(p.status)[0] : (typeof p.status === 'string' ? p.status : 'stopped');
    const map = { running:['running','运行中'], stopped:['stopped','已停止'], starting:['starting','启动中'], failed:['failed','失败'], restarting:['restarting','重启中'] };
    const m = map[raw] || ['stopped', raw];
    const detail = isObj && raw === 'failed' ? ': ' + (p.status.failed || Object.values(p.status)[0] || '') : '';
    return { key: m[0], label: m[1] + detail };
}

function toast(msg, type = 'info') {
    const el = document.createElement('div');
    el.className = `toast ${type}`;
    el.textContent = msg;
    document.getElementById('toast-container').appendChild(el);
    setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; setTimeout(() => el.remove(), 300); }, 3000);
}

function connectWS() {
    // 切节点时先摘掉旧连接的回调，避免它按旧节点重连出一串幽灵连接。
    if (ws) {
        ws.onclose = null;
        ws.onmessage = null;
        ws.close();
    }
    const node = currentNode;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const params = new URLSearchParams();
    if (API_KEY) params.set('token', API_KEY);
    if (node !== 'local') params.set('node', node);
    const query = params.toString() ? `?${params}` : '';
    const socket = new WebSocket(`${proto}//${location.host}${API}/ws${query}`);
    ws = socket;
    socket.onmessage = (ev) => {
        try {
            const d = JSON.parse(ev.data);
            if (d.type === 'status') {
                allProcesses = d.processes || [];
                systemTraffic = d.system || null;
                updateNetStats();
                render();
            } else if (d.type === 'logs') {
                if (d.entries?.length) {
                    appendLogEntries(d.entries);
                }
            }
        } catch {}
    };
    socket.onclose = () => {
        if (ws === socket && currentNode === node) setTimeout(connectWS, 3000);
    };
    socket.onerror = () => socket.close();
}

function showLogin() {
    const el = document.getElementById('login-backdrop');
    if (!el) return;
    el.classList.add('open');
    setTimeout(() => {
        const inp = document.getElementById('login-key');
        if (inp) inp.focus();
    }, 100);
}

window.toggleLoginVis = function() {
    const inp = document.getElementById('login-key');
    const open = document.querySelector('.login-toggle-vis .eye-open');
    const closed = document.querySelector('.login-toggle-vis .eye-closed');
    if (inp.type === 'password') {
        inp.type = 'text';
        open.style.display = 'none';
        closed.style.display = 'block';
    } else {
        inp.type = 'password';
        open.style.display = 'block';
        closed.style.display = 'none';
    }
};
