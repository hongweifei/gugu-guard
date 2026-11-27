// 文件管理视图：白名单内浏览、预览、编辑，以及上传、下载与增删改。
// 列表与预览同屏：左侧目录，右侧文本内容；只读模式下写入口全部关闭。

import { $, busy } from '../ui/dom.js';
import { api, apiRaw, rawError } from '../core/api.js';
import { esc, fmtBytes, fmtDateTime } from '../core/format.js';
import { confirmAction } from '../ui/dialog.js';
import { promptText } from '../ui/prompt.js';
import { toast } from '../ui/toast.js';

/// 当前目录、上一级、写权限与根目录清单。
let current = '';
let upTarget = null;
let writable = false;
let roots = [];

/// 最近一次渲染的条目表与当前预览的文件；`dirty` 标记编辑器有未保存的改动。
let entries = [];
let selected = null;
let dirty = false;

/// 请求序号：目录切换与预览读取互相作废，晚到的响应不覆盖新状态。
let seq = 0;
let loaded = false;

export function initFiles() {
    $('files-up').addEventListener('click', goUp);
    $('files-refresh').addEventListener('click', reload);
    $('files-upload').addEventListener('click', () => $('files-input').click());
    $('files-mkdir').addEventListener('click', () => createEntry('dir'));
    $('files-touch').addEventListener('click', () => createEntry('file'));
    $('files-root').addEventListener('change', (event) => open(event.target.value));
    $('files-input').addEventListener('change', (event) => {
        const picked = Array.from(event.target.files || []);
        event.target.value = '';
        upload(picked);
    });
    $('ftable-body').addEventListener('click', onRowClick);
    $('files-editor').addEventListener('input', () => {
        dirty = true;
        updateSaveState();
    });
    $('files-save').addEventListener('click', save);
    $('files-download').addEventListener('click', () => {
        if (selected) download(selected);
    });
}

/// 切换到文件视图：首次进入读根目录清单，之后每次进入刷新当前目录。
export function filesActivate() {
    if (!loaded) {
        loaded = true;
        loadRoots().then(reload);
        return;
    }
    reload();
}

/// 离开文件视图：作废在途请求，避免回来时被旧响应覆盖。
export function filesDeactivate() {
    seq += 1;
}

/// 切换节点：每台机器的白名单与路径都不同，回到默认根目录重新开始。
export function filesReset() {
    seq += 1;
    current = '';
    upTarget = null;
    entries = [];
    roots = [];
    loaded = false;
    clearPreview();
}

async function loadRoots() {
    try {
        const data = await api('GET', '/fs/roots');
        roots = data?.roots || [];
        writable = Boolean(data?.writable);
        renderRoots();
        renderMode();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '读取文件根目录失败', 'error');
    }
}

function renderRoots() {
    const select = $('files-root');
    select.replaceChildren();
    for (const root of roots) {
        select.append(new Option(rootLabel(root), root));
    }
    // 只有一个根时下拉没有信息量，隐藏后由面包屑承担全部导航。
    select.hidden = roots.length < 2;
}

function renderMode() {
    const note = $('files-mode');
    note.hidden = writable;
    note.textContent = '只读模式：在 gugu.toml 的 [daemon.web] 里把 fs_write 设为 true 后重启守护进程，才能上传、新建、改名与删除。';
    for (const id of ['files-upload', 'files-mkdir', 'files-touch']) {
        $(id).disabled = !writable;
    }
}

async function reload() {
    const ticket = ++seq;
    try {
        const query = current ? `?path=${encodeURIComponent(current)}` : '';
        const listing = await api('GET', `/fs/list${query}`);
        if (ticket !== seq) return;
        current = listing.path;
        upTarget = listing.parent || null;
        writable = Boolean(listing.writable);
        renderMode();
        syncRootSelect();
        renderCrumbs(listing.path);
        renderEntries(listing.entries || []);
        $('files-up').disabled = !upTarget;
    } catch (error) {
        if (ticket !== seq) return;
        if (error.message === 'Unauthorized') return;
        toast(error.message || '读取目录失败', 'error');
    }
}

function open(path) {
    if (!path || path === current) return;
    current = path;
    clearPreview();
    reload();
}

function goUp() {
    if (upTarget) open(upTarget);
}

function syncRootSelect() {
    const root = roots.find((item) => isUnder(current, item));
    if (root) $('files-root').value = root;
}

function renderCrumbs(path) {
    const nav = $('files-crumbs');
    nav.replaceChildren();
    const items = crumbs(path);
    items.forEach((item, index) => {
        if (index) nav.append(span('crumb-sep', '/'));
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'crumb';
        button.textContent = item.name;
        button.title = item.path;
        if (index === items.length - 1) button.setAttribute('aria-current', 'page');
        else button.addEventListener('click', () => open(item.path));
        nav.append(button);
    });
    nav.title = path;
    $('files-up').title = upTarget ? `上一级：${upTarget}` : '已在根目录';
}

function renderEntries(list) {
    entries = list;
    const body = $('ftable-body');
    const table = $('ftable');
    const empty = $('files-empty');
    if (!list.length) {
        body.replaceChildren();
        table.hidden = true;
        empty.hidden = false;
        empty.innerHTML = '<h3>这个目录是空的</h3><p>用上方的「上传」「新建目录」或「新建文件」开始</p>';
        return;
    }
    table.hidden = false;
    empty.hidden = true;
    body.innerHTML = list.map((entry, index) => rowHtml(entry, index)).join('');
}

function rowHtml(entry, index) {
    const kind = entry.kind === 'dir' ? 'dir' : (entry.kind === 'link' ? 'link' : 'file');
    const size = entry.is_dir ? '—' : fmtBytes(entry.size);
    const actions = [];
    if (!entry.is_dir) actions.push(actionButton('download', '下载', false));
    actions.push(actionButton('rename', '改名', !writable));
    actions.push(actionButton('copy', '复制', !writable));
    // 白名单根自身不提供删除：删掉根等于把整个视图删空。
    if (writable && !isRoot(entry.path)) actions.push(actionButton('remove', '删除', false, true));
    const mark = entry.readonly ? '<span class="badge badge-idle">只读</span>' : '';
    return `<tr data-index="${index}">
        <td class="ft-name">
            <button type="button" class="ft-open" data-act="open">
                <i class="ft-icon ${kind}"></i><span>${esc(entry.name)}</span>${mark}
            </button>
        </td>
        <td class="ft-size mono">${size}</td>
        <td class="ft-time mono">${esc(fmtDateTime(entry.modified))}</td>
        <td class="ft-actions">${actions.join('')}</td>
    </tr>`;
}

function actionButton(act, label, disabled, danger = false) {
    const cls = danger ? 'btn btn-sm btn-quiet btn-danger-text' : 'btn btn-sm btn-quiet';
    return `<button type="button" class="${cls}" data-act="${act}"${disabled ? ' disabled' : ''}>${label}</button>`;
}

function onRowClick(event) {
    const button = event.target.closest('button[data-act]');
    if (!button) return;
    const row = button.closest('tr');
    if (!row) return;
    const entry = entries[Number(row.dataset.index)];
    if (!entry) return;
    const act = button.dataset.act;
    if (act === 'open') {
        if (entry.is_dir) open(entry.path);
        else openFile(entry);
        return;
    }
    if (act === 'download') download(entry);
    if (act === 'rename') renameEntry(entry);
    if (act === 'copy') copyEntry(entry);
    if (act === 'remove') removeEntry(entry);
}

// ── 预览与编辑 ─────────────────────────────────────────

async function openFile(entry) {
    const ticket = ++seq;
    try {
        const file = await api('GET', `/fs/read?path=${encodeURIComponent(entry.path)}`);
        if (ticket !== seq) return;
        renderPreview(entry, file);
    } catch (error) {
        if (ticket !== seq) return;
        if (error.message !== 'Unauthorized') toast(error.message || '读取文件失败', 'error');
    }
}

function renderPreview(entry, file) {
    selected = entry;
    dirty = false;
    $('files-preview-name').textContent = entry.name;
    $('files-preview-name').title = entry.path;
    const meta = [fmtBytes(file.size)];
    if (!file.binary) meta.push(file.encoding.toUpperCase());
    if (file.truncated) meta.push('只显示开头 1 MB');
    $('files-preview-meta').textContent = meta.join(' · ');

    $('files-download').hidden = false;
    const editor = $('files-editor');

    if (file.binary) {
        editor.hidden = true;
        editor.value = '';
        $('files-save').hidden = true;
        $('files-hint').hidden = false;
        $('files-hint').textContent = '这是二进制文件，不提供文本预览；用「下载」取回原文件。';
        return;
    }

    editor.hidden = false;
    editor.value = file.content;
    // 截断的预览只读了开头，直接保存会丢掉尾部，因此这种情况下不开放编辑。
    const editable = writable && !file.truncated;
    editor.readOnly = !editable;
    editor.classList.toggle('readonly', !editable);
    $('files-hint').hidden = true;
    $('files-save').hidden = !editable;
    updateSaveState();
}

function clearPreview() {
    selected = null;
    dirty = false;
    $('files-preview-name').textContent = '未选择文件';
    $('files-preview-name').title = '';
    $('files-preview-meta').textContent = '';
    $('files-download').hidden = true;
    $('files-save').hidden = true;
    const editor = $('files-editor');
    editor.hidden = true;
    editor.value = '';
    $('files-hint').hidden = false;
    $('files-hint').textContent = '选择左侧文件查看内容';
}

function updateSaveState() {
    const button = $('files-save');
    button.disabled = !dirty;
    button.textContent = dirty ? '保存' : '已保存';
}

async function save() {
    if (!selected || !dirty) return;
    const button = $('files-save');
    const path = selected.path;
    busy(button, true);
    try {
        const result = await api('PUT', `/fs/write?path=${encodeURIComponent(path)}`, {
            path,
            content: $('files-editor').value,
            create: false,
        });
        dirty = false;
        toast(result?.message || '已保存', 'success');
        await reload();
        const entry = entries.find((item) => item.path === path);
        if (entry) await openFile(entry);
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '保存失败', 'error');
    } finally {
        busy(button, false);
        updateSaveState();
    }
}

// ── 下载与上传 ─────────────────────────────────────────

async function download(entry) {
    try {
        const resp = await apiRaw('GET', `/fs/download?path=${encodeURIComponent(entry.path)}`);
        if (!resp.ok) throw await rawError(resp);
        const blob = await resp.blob();
        const url = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = url;
        link.download = entry.name;
        document.body.append(link);
        link.click();
        link.remove();
        URL.revokeObjectURL(url);
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '下载失败', 'error');
    }
}

async function upload(files) {
    if (!files.length) return;
    if (!writable) {
        toast('当前是只读模式，无法上传', 'info');
        return;
    }
    for (const file of files) {
        const target = join(current, file.name);
        try {
            await sendUpload(target, file, false);
            toast(`已上传 ${file.name}`, 'success');
            continue;
        } catch (error) {
            if (error.message === 'Unauthorized') return;
            if (!isExistsError(error)) {
                toast(error.message || `上传 ${file.name} 失败`, 'error');
                continue;
            }
        }

        const ok = await confirmAction({
            title: '覆盖同名文件',
            text: `「${target}」已经存在，确认用上传的内容覆盖它？`,
            okText: '覆盖',
            danger: true,
        });
        if (!ok) continue;
        try {
            await sendUpload(target, file, true);
            toast(`已覆盖 ${file.name}`, 'success');
        } catch (error) {
            if (error.message === 'Unauthorized') return;
            toast(error.message || `覆盖 ${file.name} 失败`, 'error');
        }
    }
    await reload();
}

async function sendUpload(target, file, overwrite) {
    const url = `/fs/upload?path=${encodeURIComponent(target)}&overwrite=${overwrite}`;
    const resp = await apiRaw('POST', url, { body: file });
    if (!resp.ok) throw await rawError(resp);
    return resp.json().catch(() => null);
}

/// 上传冲突由服务端按 400 返回，消息里带「已存在」。
function isExistsError(error) {
    return String(error?.message || '').includes('已存在');
}

// ── 新建、改名、复制与删除 ─────────────────────────────

async function createEntry(kind) {
    if (!writable) return;
    const dir = kind === 'dir';
    const name = await promptText({
        title: dir ? '新建目录' : '新建文件',
        label: dir ? '目录名（可以写多级，用 / 分隔）' : '文件名',
        okText: '创建',
        hint: `创建在 ${current}`,
    });
    if (!name) return;
    const path = join(current, name);
    // 路径同时放进查询串：审计中间件在读完请求体之前就要记下对象。
    const url = dir
        ? `/fs/mkdir?path=${encodeURIComponent(path)}`
        : `/fs/touch?path=${encodeURIComponent(path)}`;
    try {
        const result = dir
            ? await api('POST', url, { path, parents: true })
            : await api('POST', url, { path });
        toast(result?.message || (dir ? '目录已创建' : '文件已创建'), 'success');
        await reload();
        if (!dir) {
            const entry = entries.find((item) => item.path === result?.path);
            if (entry) await openFile(entry);
        }
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '创建失败', 'error');
    }
}

async function renameEntry(entry) {
    const to = await promptText({
        title: '重命名 / 移动',
        label: '新路径',
        value: entry.path,
        okText: '提交',
        hint: '只改名字就写文件名；写完整路径可以把条目移动到别的目录。',
    });
    if (!to || to === entry.path) return;
    try {
        const url = `/fs/rename?from=${encodeURIComponent(entry.path)}`;
        const result = await relocate(url, entry.path, to);
        if (!result) return;
        toast(`已重命名为 ${to}`, 'success');
        if (entry.path === current) open(parentOf(to));
        else await reload();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '重命名失败', 'error');
    }
}

async function copyEntry(entry) {
    const to = await promptText({
        title: '复制到',
        label: '目标路径',
        value: `${entry.path}.copy`,
        okText: '复制',
        hint: '目标已存在时会再确认一次是否覆盖。',
    });
    if (!to || to === entry.path) return;
    try {
        const url = `/fs/copy?from=${encodeURIComponent(entry.path)}`;
        const result = await relocate(url, entry.path, to);
        if (!result) return;
        toast(`已复制到 ${to}`, 'success');
        await reload();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '复制失败', 'error');
    }
}

/// 改名与复制共用「先按不覆盖提交，冲突时确认后重试」的流程；用户取消时返回 null。
async function relocate(url, from, to) {
    try {
        return await api('POST', url, { from, to, overwrite: false });
    } catch (error) {
        if (error.message === 'Unauthorized' || !isExistsError(error)) throw error;
    }
    const ok = await confirmAction({
        title: '覆盖目标',
        text: `「${to}」已经存在，确认覆盖？`,
        okText: '覆盖',
        danger: true,
    });
    if (!ok) return null;
    return api('POST', url, { from, to, overwrite: true });
}

async function removeEntry(entry) {
    const ok = await confirmAction({
        title: '删除',
        text: entry.is_dir
            ? `确认删除目录「${entry.name}」？目录内的全部内容会一并删除，无法撤销。`
            : `确认删除文件「${entry.name}」？此操作无法撤销。`,
        okText: '删除',
        danger: true,
    });
    if (!ok) return;
    try {
        const url = `/fs?path=${encodeURIComponent(entry.path)}&recursive=${entry.is_dir}`;
        const result = await api('DELETE', url);
        toast(result?.message || '已删除', 'success');
        if (selected?.path === entry.path) clearPreview();
        await reload();
    } catch (error) {
        if (error.message !== 'Unauthorized') toast(error.message || '删除失败', 'error');
    }
}

// ── 路径工具 ───────────────────────────────────────────

function join(dir, name) {
    if (!dir) return name;
    return `${dir.replace(/[\\/]+$/, '')}/${name.replace(/^[\\/]+/, '')}`;
}

function parentOf(path) {
    const trimmed = path.replace(/[\\/]+$/, '');
    const index = trimmed.lastIndexOf('/');
    return index > 0 ? trimmed.slice(0, index) : '';
}

function isUnder(path, root) {
    const normalize = (value) => value.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
    const target = normalize(path);
    const base = normalize(root);
    return target === base || target.startsWith(`${base}/`);
}

function isRoot(path) {
    return roots.some((root) => isUnder(path, root) && isUnder(root, path));
}

function rootLabel(root) {
    const parts = root.replace(/[\\/]+$/, '').split('/').filter(Boolean);
    return parts.length ? parts[parts.length - 1] : root;
}

/// 面包屑：白名单内的路径从根开始，没有白名单时按绝对路径本身分层。
function crumbs(path) {
    const root = roots.find((item) => isUnder(path, item));
    const items = [];
    let acc = '';
    if (root) {
        items.push({ name: rootLabel(root), path: root });
        acc = root;
    }
    const rest = root ? path.slice(root.length) : path;
    for (const part of rest.split('/').filter(Boolean)) {
        if (!acc) acc = /^[A-Za-z]:$/.test(part) ? part : `/${part}`;
        else acc = `${acc}/${part}`;
        items.push({ name: part, path: acc });
    }
    return items;
}

function span(className, text) {
    const node = document.createElement('span');
    node.className = className;
    node.textContent = text;
    return node;
}
