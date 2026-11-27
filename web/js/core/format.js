// 展示层格式化：字节、速率、时长、时间与状态文案。
// 只做纯转换，不读状态、不碰 DOM。

export function esc(value) {
    return String(value ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

/// 二进制单位（1024 进制），非 B 级别保留一位小数，与 `/metrics` 口径分开：
/// 面板给人读，指标端点给程序读。
export function fmtBytes(n) {
    if (n == null || Number.isNaN(n)) return '—';
    const KB = 1024;
    const MB = KB * 1024;
    const GB = MB * 1024;
    if (n >= GB) return `${(n / GB).toFixed(2)} GB`;
    if (n >= MB) return `${(n / MB).toFixed(1)} MB`;
    if (n >= KB) return `${(n / KB).toFixed(1)} KB`;
    return `${Math.round(n)} B`;
}

export function fmtRate(n) {
    if (n == null || Number.isNaN(n)) return '—';
    if (n < 1024) return `${Math.round(n)} B/s`;
    return `${fmtBytes(n)}/s`;
}

/// 紧凑时长：`3d 4h` / `2h 14m` / `5m 20s` / `12s`。表格与 KPI 共用。
export function fmtDuration(secs) {
    if (secs == null || Number.isNaN(secs)) return '—';
    const s = Math.max(0, Math.floor(secs));
    if (s >= 86400) return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
    if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
    if (s >= 60) return `${Math.floor(s / 60)}m ${s % 60}s`;
    return `${s}s`;
}

const pad2 = (n) => String(n).padStart(2, '0');

export function fmtClock(value) {
    if (!value) return '—';
    const d = new Date(value);
    if (Number.isNaN(d.getTime())) return '—';
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

export function fmtDateTime(value) {
    if (!value) return '—';
    const d = new Date(value);
    if (Number.isNaN(d.getTime())) return String(value);
    return `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

const STATUS = {
    running: { label: '运行中', tone: 'ok' },
    stopped: { label: '已停止', tone: 'idle' },
    starting: { label: '启动中', tone: 'info' },
    restarting: { label: '重启中', tone: 'violet' },
    failed: { label: '失败', tone: 'bad' },
};

/// 服务端把状态序列化成字符串或 `{ failed: "原因" }`，两种都还原成统一形状。
export function statusMeta(p) {
    const isObject = p && typeof p.status === 'object' && p.status !== null;
    const raw = isObject
        ? Object.keys(p.status)[0]
        : (typeof p?.status === 'string' ? p.status : 'stopped');
    const base = STATUS[raw] || { label: raw, tone: 'idle' };
    const detail = isObject && raw === 'failed'
        ? String(p.status.failed ?? Object.values(p.status)[0] ?? '')
        : '';
    return {
        key: STATUS[raw] ? raw : 'stopped',
        label: base.label,
        tone: base.tone,
        detail,
        text: detail ? `${base.label}：${detail}` : base.label,
    };
}

export function commandLine(p) {
    if (!p) return '';
    const args = Array.isArray(p.args) ? p.args : [];
    return args.length ? `${p.command || ''} ${args.join(' ')}`.trim() : (p.command || '');
}

const LEVEL_RANK = { trace: 0, debug: 1, info: 2, warn: 3, error: 4, fatal: 5 };

export function levelRank(level) {
    return level != null && LEVEL_RANK[level] != null ? LEVEL_RANK[level] : -1;
}

/// 输出流有字符串与 `{ Stdout: null }` 两种序列化形式。
export function normStream(stream) {
    if (typeof stream === 'string') return stream.toLowerCase();
    return stream && stream.Stdout != null ? 'stdout' : 'stderr';
}

export function auditActor(entry) {
    const ua = entry?.user_agent || '';
    if (ua.startsWith('gugu-cli/')) return `CLI ${ua.slice(9)}`;
    if (ua.includes('PowerShell')) return 'PowerShell';
    if (ua.includes('Mozilla')) return 'WebUI';
    if (!ua) return '—';
    return ua.length > 26 ? `${ua.slice(0, 26)}…` : ua;
}

/// 逗号/分号/空白分隔的片段列表，去空去重。表单里的关键字、依赖都走这里。
export function parseList(value, separator = /[,;\s]+/) {
    if (!value) return [];
    const parts = String(value).split(separator).map((item) => item.trim()).filter(Boolean);
    return [...new Set(parts)];
}

