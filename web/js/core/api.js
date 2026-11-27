// HTTP 传输：统一拼节点前缀、带认证头、把 401 交给登录流程。
// 业务模块只调用 `api` / `hubApi`，不自行拼 URL。

import { state } from './state.js';

export const API_BASE = '/api/v1';

/// 把节点标识编成 URL 路径段：保留字母数字与 `-` `_`，其余字节逐个转义。
/// 点段由 `safeNodeId` 在进路径之前挡掉（`%2E%2E` 仍会被 URL 解析折叠）。
export function nodeSegment(id) {
    return Array.from(new TextEncoder().encode(id)).map((b) => {
        const ch = String.fromCharCode(b);
        return /[A-Za-z0-9_-]/.test(ch) ? ch : `%${b.toString(16).toUpperCase().padStart(2, '0')}`;
    }).join('');
}

/// 本机不加前缀，从节点走中心节点的代理路径。
export function apiPath(path) {
    return state.node === 'local' ? path : `/nodes/${nodeSegment(state.node)}${path}`;
}

let unauthorizedHandler = () => {};

export function onUnauthorized(fn) {
    unauthorizedHandler = fn;
}

export async function request(method, path, body) {
    const opts = { method };
    if (body !== undefined) {
        opts.headers = { 'Content-Type': 'application/json' };
        opts.body = JSON.stringify(body);
    }
    if (state.apiKey) {
        opts.headers = { ...(opts.headers || {}), Authorization: `Bearer ${state.apiKey}` };
    }

    const resp = await fetch(`${API_BASE}${path}`, opts);
    if (resp.status === 401) {
        unauthorizedHandler();
        throw new Error('Unauthorized');
    }

    const text = await resp.text();
    let data = null;
    if (text) {
        try {
            data = JSON.parse(text);
        } catch {
            data = { error: text };
        }
    }
    if (!resp.ok) {
        throw new Error(data?.error || `请求失败（HTTP ${resp.status}）`);
    }
    return data;
}

/// 业务接口：自动跟随当前节点。
export function api(method, path, body) {
    return request(method, apiPath(path), body);
}

/// 原始响应：上传与下载这类非 JSON 传输用，调用方自己读 body 并处理状态码。
/// 认证与节点前缀的拼接口径与 `api` 一致。
export async function apiRaw(method, path, { body, headers } = {}) {
    const opts = { method };
    const merged = { ...(headers || {}) };
    if (state.apiKey) merged.Authorization = `Bearer ${state.apiKey}`;
    if (Object.keys(merged).length) opts.headers = merged;
    if (body !== undefined) opts.body = body;

    const resp = await fetch(`${API_BASE}${apiPath(path)}`, opts);
    if (resp.status === 401) {
        unauthorizedHandler();
        throw new Error('Unauthorized');
    }
    return resp;
}

/// 把非 JSON 响应的正文读成一句错误消息，供 `apiRaw` 的调用方直接抛出。
export async function rawError(resp) {
    const text = await resp.text().catch(() => '');
    try {
        const data = JSON.parse(text);
        return new Error(data?.error || `请求失败（HTTP ${resp.status}）`);
    } catch {
        return new Error(text || `请求失败（HTTP ${resp.status}）`);
    }
}

/// 上传：`fetch` 拿不到上传进度，这里用 XMLHttpRequest；认证与节点前缀口径与 `api` 一致。
export function uploadXhr(method, path, blob, { onProgress, signal } = {}) {
    return new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open(method, `${API_BASE}${apiPath(path)}`);
        if (state.apiKey) xhr.setRequestHeader('Authorization', `Bearer ${state.apiKey}`);
        xhr.upload.onprogress = (event) => {
            if (event.lengthComputable) onProgress?.(event.loaded, event.total);
        };
        xhr.onload = () => {
            let data = null;
            try {
                data = JSON.parse(xhr.responseText);
            } catch {
                data = null;
            }
            if (xhr.status >= 200 && xhr.status < 300) {
                resolve(data);
                return;
            }
            if (xhr.status === 401) {
                unauthorizedHandler();
                reject(new Error('Unauthorized'));
                return;
            }
            reject(new Error(data?.error || `请求失败（HTTP ${xhr.status}）`));
        };
        xhr.onerror = () => reject(new Error('传输中断'));
        xhr.onabort = () => reject(Object.assign(new Error('已取消'), { name: 'AbortError' }));
        if (signal) {
            if (signal.aborted) {
                xhr.abort();
                return;
            }
            signal.addEventListener('abort', () => xhr.abort(), { once: true });
        }
        xhr.send(blob);
    });
}

/// 下载：返回原始响应，正文由调用方逐块读取，边读边算进度。
export async function fetchStream(path, { signal } = {}) {
    const headers = {};
    if (state.apiKey) headers.Authorization = `Bearer ${state.apiKey}`;
    const resp = await fetch(`${API_BASE}${apiPath(path)}`, { headers, signal });
    if (resp.status === 401) {
        unauthorizedHandler();
        throw new Error('Unauthorized');
    }
    if (!resp.ok) throw await rawError(resp);
    return resp;
}

/// 只发给中心节点自身的接口（节点表），不跟随所选节点。
export function hubApi(method, path, body) {
    return request(method, path, body);
}
