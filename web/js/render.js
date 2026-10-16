// 进程列表渲染与单进程操作。

function updateStats(list) {
    const r = list.filter(p => statusOf(p).key === 'running').length;
    const s = list.filter(p => statusOf(p).key === 'stopped').length;
    const f = list.filter(p => statusOf(p).key === 'failed').length;
    document.getElementById('stat-total').textContent = list.length;
    document.getElementById('stat-running').textContent = r;
    document.getElementById('stat-stopped').textContent = s;
    document.getElementById('stat-failed').textContent = f;
}

function getGroups() {
    const map = {};
    for (const p of allProcesses) {
        if (p.group) {
            map[p.group] = (map[p.group] || 0) + 1;
        }
    }
    return Object.entries(map).sort((a, b) => a[0].localeCompare(b[0]));
}

function renderGroupBar() {
    const bar = document.getElementById('group-bar');
    const groups = getGroups();
    if (!groups.length) {
        bar.innerHTML = '';
        bar._prevHtml = '';
        return;
    }
    const total = allProcesses.length;
    const allActive = !groupFilter;
    let html = `<button class="group-chip${allActive ? ' active' : ''}" data-group="">全部<span class="group-chip-count">${total}</span></button>`;
    for (const [name, count] of groups) {
        const active = groupFilter === name;
        html += `<span class="group-chip${active ? ' active' : ''}" data-group="${esc(name)}">
            <span class="group-chip-name">${esc(name)}</span><span class="group-chip-count">${count}</span>
            <span class="group-chip-actions">
                <button class="gact-start" data-gact="start" data-gname="${esc(name)}" title="启动组 ${esc(name)}">&#9654;</button>
                <button class="gact-stop" data-gact="stop" data-gname="${esc(name)}" title="停止组 ${esc(name)}">&#9632;</button>
                <button class="gact-restart" data-gact="restart" data-gname="${esc(name)}" title="重启组 ${esc(name)}">&#8635;</button>
            </span>
        </span>`;
    }
    if (html !== bar._prevHtml) {
        bar.innerHTML = html;
        bar._prevHtml = html;
    }
}

async function doGroupAction(action, group) {
    try {
        const r = await api('POST', `/groups/${encodeURIComponent(group)}/${action}`);
        toast(r.message || '操作完成', r.failed > 0 ? 'info' : 'success');
    } catch { toast('组操作失败', 'error'); }
}

let prevKeys = [];

function render(force) {
    const grid = document.getElementById('process-grid');
    const list = searchQuery
        ? allProcesses.filter(p => p.name.toLowerCase().includes(searchQuery) || (p.command || '').toLowerCase().includes(searchQuery) || (p.group || '').toLowerCase().includes(searchQuery))
        : allProcesses;

    const filtered = groupFilter ? list.filter(p => p.group === groupFilter) : list;

    updateStats(allProcesses);
    renderGroupBar();

    const keys = filtered.map(p => p.name);

    if (!filtered.length) {
        const isSearch = searchQuery && allProcesses.length > 0;
        grid.innerHTML = `<div class="empty-state">
            <div class="empty-icon"><svg viewBox="0 0 80 80" fill="none" stroke="currentColor" stroke-width="1.5">
                ${isSearch
                    ? '<circle cx="35" cy="35" r="18"/><line x1="48" y1="48" x2="65" y2="65" stroke-width="4" stroke-linecap="round"/>'
                    : '<rect x="10" y="20" width="60" height="40" rx="6"/><circle cx="30" cy="40" r="6"/><circle cx="50" cy="40" r="6"/><line x1="1" y1="40" x2="10" y2="40"/><line x1="70" y1="40" x2="79" y2="40"/>'}
            </svg></div>
            <h3>${isSearch ? '没有匹配的进程' : '暂无受管进程'}</h3>
            <p>${isSearch ? '尝试其他关键词' : '点击上方「添加进程」开始管理你的服务'}</p>
        </div>`;
        prevKeys = [];
        return;
    }

    if (!force && keys.join(',') === prevKeys.join(',')) {
        for (const p of filtered) {
            const card = grid.querySelector(`[data-name="${CSS.escape(p.name)}"]`);
            if (!card) continue;
            patchCard(card, p);
        }
        return;
    }

    prevKeys = keys;
    grid.innerHTML = filtered.map((p, i) => buildCard(p, i === 0)).join('');
}

function fmtBytes(n) {
    const KB = 1024, MB = KB * 1024, GB = MB * 1024;
    if (n >= GB) return (n / GB).toFixed(2) + ' GB';
    if (n >= MB) return (n / MB).toFixed(1) + ' MB';
    if (n >= KB) return (n / KB).toFixed(1) + ' KB';
    return n + ' B';
}

function usageInfo(p) {
    const u = p.usage;
    if (!u) return { label: '—', title: '暂无采样（进程未运行）' };
    const mem = fmtBytes(u.memory_bytes);
    const cpu = `${u.cpu_percent.toFixed(1)}%`;
    const extra = u.process_count > 1 ? ` · 进程树 ${u.process_count} 个进程` : '';
    return {
        label: `${mem} · ${cpu}`,
        title: `进程树常驻内存 ${mem}，CPU 占单核 ${cpu}${extra}`,
    };
}

function lastExitInfo(p) {
    const list = p.recent_exits || [];
    if (!list.length) return { label: '—', title: '暂无退出记录' };
    const last = list[list.length - 1];
    const code = last.code == null ? '未知' : last.code;
    const t = new Date(last.at).toLocaleString();
    const label = `${code} · ${new Date(last.at).toLocaleTimeString()}${last.auto_restarted ? ' · 已自愈' : ''}`;
    const title = list.map(e => {
        const c = e.code == null ? '未知' : e.code;
        return `${new Date(e.at).toLocaleString()} 退出码 ${c}${e.auto_restarted ? ' (已自动拉起)' : ''}`;
    }).join('\n');
    return { label, title: `最近 ${list.length} 次退出：\n${title}` };
}

function buildCard(p, isNew) {
    const st = statusOf(p);
    const cmd = p.args?.length ? `${p.command} ${p.args.join(' ')}` : (p.command || '');
    const run = st.key === 'running';
    let hcHtml = '';
    if (p.has_health_check) {
        const cls = p.healthy === true ? 'healthy' : (p.healthy === false ? 'unhealthy' : 'unknown');
        const label = p.healthy === true ? '健康' : (p.healthy === false ? '异常' : '待检');
        hcHtml = `<span class="pcard-tag hc ${cls}" title="健康检查: ${label}">HC ${label}</span>`;
    }
    const urTag = p.unhealthy_restart ? '<span class="pcard-tag ur" title="失败自动重启">AR</span>' : '';
    const groupTag = p.group ? `<span class="pcard-tag group" data-action="filter-group" data-group="${esc(p.group)}" title="筛选组: ${esc(p.group)}">${esc(p.group)}</span>` : '';
    const hcBtn = p.has_health_check ? `<button class="btn btn-hc" data-action="health" data-name="${esc(p.name)}" title="手动健康检查">检查</button>` : '';
    const exit = lastExitInfo(p);
    const usage = usageInfo(p);
    return `<div class="pcard" data-name="${esc(p.name)}">
        <div class="pcard-head">
            <div class="pcard-name"><span class="pcard-dot ${st.key}"></span>${esc(p.name)}</div>
            <div class="pcard-badges"><span class="pcard-badge ${st.key}">${esc(st.label)}</span>${groupTag}${hcHtml}${urTag}</div>
        </div>
        <div class="pcard-meta">
            <div class="pcard-meta-item"><span class="pcard-meta-label">PID</span><span class="pcard-meta-val">${p.pid || '—'}</span></div>
            <div class="pcard-meta-item"><span class="pcard-meta-label">运行时间</span><span class="pcard-meta-val">${fmtTime(p.uptime_secs)}</span></div>
            <div class="pcard-meta-item pcard-cmd"><span class="pcard-meta-label">命令</span><span class="pcard-meta-val" title="${esc(cmd)}">${esc(cmd)}</span></div>
            <div class="pcard-meta-item pcard-exit"><span class="pcard-meta-label">上次退出</span><span class="pcard-meta-val" title="${esc(exit.title)}">${esc(exit.label)}</span></div>
            <div class="pcard-meta-item pcard-usage"><span class="pcard-meta-label">资源</span><span class="pcard-meta-val" title="${esc(usage.title)}">${esc(usage.label)}</span></div>
        </div>
        <div class="pcard-actions">
            <button class="btn act-start" data-action="start" data-name="${esc(p.name)}" ${run?'disabled':''}>启动</button>
            <button class="btn act-stop" data-action="stop" data-name="${esc(p.name)}" ${!run?'disabled':''}>停止</button>
            <button class="btn act-restart" data-action="restart" data-name="${esc(p.name)}">重启</button>
            ${hcBtn}
            <button class="btn act-logs" data-action="logs" data-name="${esc(p.name)}">日志</button>
            <button class="btn" data-action="edit" data-name="${esc(p.name)}">编辑</button>
            <button class="btn act-delete" data-action="remove" data-name="${esc(p.name)}">删除</button>
        </div>
    </div>`;
}

function patchCard(card, p) {
    const st = statusOf(p);
    const run = st.key === 'running';
    const cmd = p.args?.length ? `${p.command} ${p.args.join(' ')}` : (p.command || '');

    card.querySelector('.pcard-dot').className = `pcard-dot ${st.key}`;
    card.querySelector('.pcard-badge').className = `pcard-badge ${st.key}`;
    card.querySelector('.pcard-badge').textContent = st.label;

    const metas = card.querySelectorAll('.pcard-meta-val');
    metas[0].textContent = p.pid || '—';
    metas[1].textContent = fmtTime(p.uptime_secs);
    metas[2].textContent = cmd;
    metas[2].title = cmd;
    if (metas[3]) {
        const exit = lastExitInfo(p);
        metas[3].textContent = exit.label;
        metas[3].title = exit.title;
    }
    if (metas[4]) {
        const usage = usageInfo(p);
        metas[4].textContent = usage.label;
        metas[4].title = usage.title;
    }

    const btns = card.querySelectorAll('.pcard-actions .btn');
    btns[0].disabled = run;
    btns[1].disabled = !run;
}

async function doAct(action, name) {
    try {
        const r = await api('POST', `/processes/${encodeURIComponent(name)}/${action}`);
        toast(r.message || '操作成功', 'success');
        if (r.error) toast(r.error, 'error');
    } catch { toast('操作失败', 'error'); }
}

async function doHealthCheck(name) {
    try {
        const r = await api('POST', `/processes/${encodeURIComponent(name)}/health`);
        if (r.healthy) {
            toast('健康检查通过', 'success');
        } else {
            toast('健康检查失败', 'error');
        }
    } catch { toast('健康检查请求失败', 'error'); }
}
