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

/// 只发给中心节点自身的接口（节点表），不跟随所选节点。
export function hubApi(method, path, body) {
    return request(method, path, body);
}
