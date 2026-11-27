// 网络表单段：监听端口列表与就绪等待。端口用逗号或空白分隔，解析成数字数组。

import { checked, number, setChecked, setText, text } from './fields.js';

export function initNetwork() {
    // 端口输入不做即时校验：保存时由接口给出权威错误，前端只负责解析。
}

export function resetNetwork() {
    setText('pf-ports', '');
    setText('pf-ready-timeout', '60');
    setChecked('pf-port-check', true);
}

export function fillNetwork(config) {
    setText('pf-ports', (config.listen_ports || []).join(', '));
    setText('pf-ready-timeout', config.ready_timeout_secs ?? 60);
    setChecked('pf-port-check', config.port_check !== false);
}

export function collectNetwork() {
    return {
        listen_ports: parsePorts(text('pf-ports')),
        port_check: checked('pf-port-check'),
        ready_timeout_secs: number('pf-ready-timeout', 60),
    };
}

/// `8080, 9000` / `8080 9000` / 换行分隔都接受；无法解析的片段直接跳过。
export function parsePorts(value) {
    if (!value) return [];
    return value
        .split(/[\s,;]+/)
        .map((piece) => Number.parseInt(piece, 10))
        .filter((port) => Number.isInteger(port) && port > 0 && port <= 65535);
}
