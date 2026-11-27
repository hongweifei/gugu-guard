// 健康检查表单段：类型决定显示哪些字段；未启用时通用字段整体禁用。
// 接口里 healthy 配置由 type 决定形态：tcp 用 host/port，http 用 url。

import { $ } from '../ui/dom.js';
import { checked, number, setChecked, setText, text } from './fields.js';

const TCP_FIELDS = ['pf-hc-tcp-host', 'pf-hc-tcp-port'];
const COMMON_INPUTS = ['pf-hc-interval', 'pf-hc-timeout', 'pf-hc-fail'];

export function initHealth() {
    $('pf-hc-type').addEventListener('change', syncVisibility);
    syncVisibility();
}

/// 类型为空即关闭健康检查；通用字段随启用状态联动，避免填了值却不生效。
function syncVisibility() {
    const type = $('pf-hc-type').value;
    const tcp = type === 'tcp';
    const http = type === 'http';
    for (const id of TCP_FIELDS) $(id).hidden = !tcp;
    $('pf-hc-url').hidden = !http;

    for (const id of COMMON_INPUTS) $(id).disabled = !(tcp || http);
    $('pf-unhealthy-restart').disabled = !(tcp || http);
    $('pf-hc-common').classList.toggle('is-muted', !(tcp || http));
    $('pf-hc-restart').classList.toggle('is-muted', !(tcp || http));
}

export function resetHealth() {
    setText('pf-hc-type', '');
    setText('pf-hc-host', '127.0.0.1');
    setText('pf-hc-port', '');
    setText('pf-hc-url-input', '');
    setText('pf-hc-interval', '30');
    setText('pf-hc-timeout', '5');
    setText('pf-hc-fail', '3');
    setChecked('pf-unhealthy-restart', false);
    syncVisibility();
}

/// 把接口返回的 healthy 配置回填到表单。
export function fillHealth(health) {
    if (!health || (health.type !== 'tcp' && health.type !== 'http')) {
        resetHealth();
        return;
    }
    setText('pf-hc-type', health.type);
    setText('pf-hc-host', health.host || '127.0.0.1');
    setText('pf-hc-port', health.port ?? '');
    setText('pf-hc-url-input', health.url || '');
    setText('pf-hc-interval', health.interval_secs ?? 30);
    setText('pf-hc-timeout', health.timeout_secs ?? 5);
    setText('pf-hc-fail', health.fail_threshold ?? 3);
    syncVisibility();
}

/// 收集健康检查配置；未启用时返回 null，与接口语义一致。
export function collectHealth() {
    const type = $('pf-hc-type').value;
    if (type !== 'tcp' && type !== 'http') return null;
    const health = {
        type,
        interval_secs: number('pf-hc-interval', 30),
        timeout_secs: number('pf-hc-timeout', 5),
        fail_threshold: number('pf-hc-fail', 3),
    };
    if (type === 'tcp') {
        health.host = text('pf-hc-host') || '127.0.0.1';
        health.port = number('pf-hc-port', 0);
    } else {
        health.url = text('pf-hc-url-input');
    }
    return health;
}

export function healthEnabled() {
    const type = $('pf-hc-type').value;
    return type === 'tcp' || type === 'http';
}

export function healthyRestart() {
    return healthEnabled() && checked('pf-unhealthy-restart');
}
