// 多节点：顶栏节点选择器。切换后 REST 请求（core.js 的 apiPath）与实时推送
// （connectWS 的 node 参数）都跟随所选节点；打开着的浮层先关掉，
// 避免上一个节点的日志、指标留在屏幕上。
// 节点表来自中心节点自己的 GET /api/v1/nodes，每 10 秒刷新一次在线状态。

let nodeList = [];
let nodePollTimer = null;
let nodePickerSignature = '';

async function initNodes() {
    const select = document.getElementById('node-select');
    select.addEventListener('change', () => switchNode(select.value));
    await loadNodes();
    if (!nodePollTimer) {
        nodePollTimer = setInterval(() => { loadNodes(); }, 10000);
    }
}

async function loadNodes() {
    let body;
    try {
        body = await hubApi('GET', '/nodes');
    } catch {
        // 401 时 showLogin 已弹出；网络抖动保留上一次的节点表。
        return;
    }
    nodeList = body.nodes || [];
    if (!nodeList.some(n => n.id === currentNode)) {
        currentNode = 'local';
        localStorage.setItem('gugu_node', 'local');
    }
    renderNodePicker();
    renderNodeBanner();
    refreshTerminalEntry();
}

function renderNodePicker() {
    const picker = document.getElementById('node-picker');
    const select = document.getElementById('node-select');
    if (!nodeList.length) { picker.hidden = true; return; }
    picker.hidden = false;

    const signature = nodeList.map(n => `${n.id}:${n.name}:${n.online}:${n.local}`).join('|') + `#${currentNode}`;
    if (signature !== nodePickerSignature) {
        nodePickerSignature = signature;
        select.innerHTML = nodeList.map(n => {
            const label = n.local ? `${n.name}（本机）` : `${n.name} · ${n.online ? '在线' : '离线'}`;
            return `<option value="${esc(n.id)}"${n.id === currentNode ? ' selected' : ''}>${esc(label)}</option>`;
        }).join('');
    }

    const current = nodeList.find(n => n.id === currentNode);
    const dot = document.getElementById('node-dot');
    dot.className = `node-dot ${!current || current.local ? 'local' : (current.online ? 'online' : 'offline')}`;
}

function renderNodeBanner() {
    const banner = document.getElementById('node-banner');
    const current = nodeList.find(n => n.id === currentNode);
    if (!current || current.local || current.online) {
        banner.hidden = true;
        return;
    }
    banner.hidden = false;
    banner.innerHTML = `<span class="node-banner-dot"></span>节点「${esc(current.name)}」当前不可达：${esc(current.last_error || '连接失败')}。可以继续停留在该节点等待恢复，或切回本机。`;
}

async function switchNode(id) {
    id = safeNodeId(id);
    if (id === currentNode) return;
    currentNode = id;
    localStorage.setItem('gugu_node', id);

    closeAllOverlays();
    resetViewState();
    render(true);
    updateNetStats();

    renderNodePicker();
    renderNodeBanner();
    refreshTerminalEntry();
    connectWS();

    const current = nodeList.find(n => n.id === id);
    try {
        allProcesses = await api('GET', '/processes');
        render(true);
        toast(`已切换到节点 ${current ? current.name : id}`, 'success');
    } catch {
        render(true);
        toast(`节点 ${current ? current.name : id} 暂时读不到进程列表`, 'error');
    }
}
