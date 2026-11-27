// 实时通道：一条 WebSocket 承担状态快照（每 2 秒）与日志增量（每 200 毫秒）。
// 切换节点时重建连接；非预期断开后 3 秒重连。

import { API_BASE } from './api.js';
import { setLive, setProcesses, setTraffic, state } from './state.js';

let socket = null;
let retryTimer = null;
let logConsumer = () => {};
let statusConsumer = () => {};

/// 日志增量订阅方（日志面板）。
export function onLogEntries(fn) {
    logConsumer = fn;
}

/// 状态快照的额外订阅方（例如抽屉里的概览要跟着刷新）。
export function onStatusSnapshot(fn) {
    statusConsumer = fn;
}

export function connectLive() {
    disconnectLive();
    const node = state.node;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const params = new URLSearchParams();
    if (state.apiKey) params.set('token', state.apiKey);
    if (node !== 'local') params.set('node', node);
    const query = params.toString() ? `?${params}` : '';

    setLive('connecting');
    const ws = new WebSocket(`${proto}//${location.host}${API_BASE}/ws${query}`);
    socket = ws;

    ws.onopen = () => {
        if (socket === ws) setLive('open');
    };

    ws.onmessage = (event) => {
        if (socket !== ws) return;
        let msg;
        try {
            msg = JSON.parse(event.data);
        } catch {
            return;
        }
        if (msg.type === 'status') {
            setProcesses(msg.processes || []);
            setTraffic(msg.system || null);
            statusConsumer();
        } else if (msg.type === 'logs' && msg.entries?.length) {
            logConsumer(msg.entries);
        }
    };

    ws.onerror = () => ws.close();

    ws.onclose = () => {
        if (socket !== ws) return;
        socket = null;
        setLive('closed');
        // 只有仍停留在同一个节点时才重连：切换节点会主动断开。
        if (state.node === node) {
            clearTimeout(retryTimer);
            retryTimer = setTimeout(connectLive, 3000);
        }
    };
}

export function disconnectLive() {
    clearTimeout(retryTimer);
    retryTimer = null;
    if (socket) {
        const ws = socket;
        socket = null;
        ws.onclose = null;
        ws.onmessage = null;
        ws.close();
    }
}

/// 告知服务端「开始关注这个进程的日志」；日志本身已经全量广播，这里只是提前订阅。
export function subscribeProcess(name) {
    if (socket?.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: 'subscribe', process: name }));
    }
}
