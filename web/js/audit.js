// 操作审计面板：读取 /api/v1/audit，按动作 / 时间范围 / 关键字 / 结果筛选。
// 数据只来自控制面写操作，不包含进程输出。

let auditEntries = [];
let auditSearchTimer = null;
let auditRequestSeq = 0;

function fmtAuditTime(at) {
    if (!at) return '-';
    const d = new Date(at);
    if (Number.isNaN(d.getTime())) return String(at);
    const pad = (n) => String(n).padStart(2, '0');
    return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function auditActor(entry) {
    const ua = entry.user_agent || '';
    if (ua.startsWith('gugu-cli/')) return `CLI (${ua.slice(9)})`;
    if (ua.includes('PowerShell')) return 'PowerShell';
    if (ua.includes('Mozilla')) return 'WebUI';
    if (!ua) return '-';
    return ua.length > 24 ? `${ua.slice(0, 24)}…` : ua;
}

function auditQueryString() {
    const params = new URLSearchParams();
    params.set('limit', '200');
    const action = document.getElementById('audit-action-filter').value;
    if (action) params.set('action', action);
    const range = document.getElementById('audit-range-filter').value;
    if (range !== '0') params.set('since_minutes', range);
    const query = document.getElementById('audit-search').value.trim();
    if (query) params.set('q', query);
    if (document.getElementById('audit-failed').checked) params.set('ok', 'false');
    return params.toString();
}

async function loadAudit() {
    const status = document.getElementById('audit-status');
    const seq = ++auditRequestSeq;
    try {
        const data = await api('GET', `/audit?${auditQueryString()}`);
        if (seq !== auditRequestSeq) return;
        auditEntries = data.entries || [];
        renderAudit(data);
        status.hidden = true;
    } catch (e) {
        if (seq !== auditRequestSeq) return;
        status.hidden = false;
        status.classList.add('history');
        status.textContent = `读取审计记录失败：${e.message || e}`;
    }
}

function renderAudit(data) {
    const body = document.getElementById('audit-body');
    const note = document.getElementById('audit-note');
    note.textContent = `记录保留 ${data.retention_days ?? 90} 天`;
    if (data.file) note.title = data.file;

    if (!auditEntries.length) {
        body.innerHTML = '<div class="log-empty">暂无操作记录。只有会改变状态的写操作（启停、增删改、重载、停止守护）才会记入审计。</div>';
        return;
    }

    const rows = auditEntries.map((entry) => {
        const ok = entry.ok;
        const result = ok
            ? '<span class="audit-result ok">成功</span>'
            : `<span class="audit-result fail">失败 ${entry.status}</span>`;
        const target = entry.target ? esc(entry.target) : '-';
        const detail = entry.detail
            ? `<td class="audit-detail" title="${esc(entry.detail)}">${esc(entry.detail)}</td>`
            : '<td class="audit-detail">-</td>';
        return `<tr class="${ok ? '' : 'audit-row-fail'}">
            <td class="audit-time">${fmtAuditTime(entry.at)}</td>
            <td class="audit-action"><span class="audit-action-label">${esc(entry.label)}</span><code>${esc(entry.action)}</code></td>
            <td class="audit-target">${target}</td>
            <td>${result}</td>
            <td class="audit-client">${esc(entry.client || '-')}</td>
            <td class="audit-actor">${esc(auditActor(entry))}</td>
            <td class="audit-duration">${entry.duration_ms < 1 ? '<1 ms' : `${entry.duration_ms} ms`}</td>
            ${detail}
        </tr>`;
    });

    body.innerHTML = `<table class="audit-table">
        <thead><tr>
            <th>时间</th><th>操作</th><th>对象</th><th>结果</th><th>来源</th><th>客户端</th><th>耗时</th><th>备注</th>
        </tr></thead>
        <tbody>${rows.join('')}</tbody>
    </table>`;
}

window.openAudit = function() {
    document.getElementById('audit-overlay').classList.add('open');
    loadAudit();
};

document.getElementById('btn-audit').onclick = window.openAudit;
document.getElementById('btn-close-audit').onclick = () => {
    document.getElementById('audit-overlay').classList.remove('open');
};
document.getElementById('btn-refresh-audit').onclick = loadAudit;
document.getElementById('audit-action-filter').onchange = loadAudit;
document.getElementById('audit-range-filter').onchange = loadAudit;
document.getElementById('audit-failed').onchange = loadAudit;
document.getElementById('audit-search').oninput = () => {
    clearTimeout(auditSearchTimer);
    auditSearchTimer = setTimeout(loadAudit, 300);
};
