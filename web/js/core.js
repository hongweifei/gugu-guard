// 全局状态与基础工具。所有跨模块共享的可变状态只在这里声明一次；
// 其余文件只读写，不再重复声明。加载顺序见 index.html 底部的 <script> 链。

const API = '/api/v1';
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
let logLineHeight = 22;
let logVisibleCount = 0;
let logScrollTop = 0;

async function api(method, path, body) {
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
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const tokenParam = API_KEY ? `?token=${encodeURIComponent(API_KEY)}` : '';
    ws = new WebSocket(`${proto}//${location.host}${API}/ws${tokenParam}`);
    ws.onmessage = (ev) => {
        try {
            const d = JSON.parse(ev.data);
            if (d.type === 'status') {
                allProcesses = d.processes || [];
                render();
            } else if (d.type === 'logs') {
                if (d.entries?.length) {
                    appendLogEntries(d.entries);
                }
            }
        } catch {}
    };
    ws.onclose = () => setTimeout(connectWS, 3000);
    ws.onerror = () => ws.close();
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
