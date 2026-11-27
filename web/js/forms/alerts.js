// 日志告警表单段：关键字与刷屏阈值都为空时不写配置，接口也就不会做日志分析。

import { number, setText, text } from './fields.js';
import { parseList } from '../core/format.js';

export function initAlerts() {
    // 纯声明式字段，无需联动。
}

export function resetAlerts() {
    setText('pf-alert-patterns', '');
    setText('pf-alert-burst', '0');
    setText('pf-alert-window', '10');
    setText('pf-alert-cooldown', '300');
}

export function fillAlerts(alert) {
    if (!alert) {
        resetAlerts();
        return;
    }
    setText('pf-alert-patterns', (alert.patterns || []).join(', '));
    setText('pf-alert-burst', alert.burst_lines ?? 0);
    setText('pf-alert-window', alert.window_secs ?? 10);
    setText('pf-alert-cooldown', alert.cooldown_secs ?? 300);
}

/// 关键字为空且刷屏阈值为 0 时返回 null：接口把空配置当作「不分析日志」。
export function collectAlerts() {
    const patterns = parseList(text('pf-alert-patterns'));
    const burst = number('pf-alert-burst', 0);
    if (!patterns.length && burst === 0) return null;
    return {
        patterns,
        burst_lines: burst,
        window_secs: number('pf-alert-window', 10),
        cooldown_secs: number('pf-alert-cooldown', 300),
    };
}
