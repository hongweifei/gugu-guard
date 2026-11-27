// 文件管理视图：白名单内浏览、预览、编辑，以及上传、下载与增删改。
// 列表与预览同屏：左侧目录，右侧文本内容；只读模式下写入口全部关闭。

import { $, busy } from '../ui/dom.js';
import { api, fetchStream, uploadXhr } from '../core/api.js';
import { state } from '../core/state.js';
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
    $('files-upload-dir').addEventListener('click', () => $('files-dir-input').click());
    $('files-mkdir').addEventListener('click', () => createEntry('dir'));
    $('files-touch').addEventListener('click', () => createEntry('file'));
    $('files-root').addEventListener('change', (event) => open(event.target.value));
    $('files-input').addEventListener('change', (event) => {
        const picked = Array.from(event.target.files || []);
        const resume = event.target.dataset.resume === '1';
        event.target.value = '';
        delete event.target.dataset.resume;
        if (resume) {
            resumeUpload(picked[0]);
            return;
        }
        upload(picked);
    });
    $('files-dir-input').addEventListener('change', (event) => {
        const picked = Array.from(event.target.files || []);
        event.target.value = '';
        uploadDirectory(picked);
    });
    $('resume-go').addEventListener('click', () => {
        // 浏览器不保留文件句柄，续传前要重新选中同一个文件。
        $('files-input').dataset.resume = '1';
        $('files-input').click();
    });
    $('resume-drop').addEventListener('click', abortResume);
    $('transfer-cancel').addEventListener('click', () => transferTask?.cancel());
    $('ftable-body').addEventListener('click', onRowClick);
    $('files-editor').addEventListener('input', () => {
        dirty = true;
        updateSaveState();
    });
    $('files-save').addEventListener('click', save);
    $('files-download').addEventListener('click', () => {
        if (selected) download(selected);
    });
    initDrop();
}

/// 切换到文件视图：首次进入读根目录清单，之后每次进入刷新当前目录。
export function filesActivate() {
    if (!loaded) {
        loaded = true;
        loadRoots().then(() => {
            reload();
            showResume();
        });
        return;
    }
    reload();
    showResume();
}

/// 离开文件视图：作废在途请求，避免回来时被旧响应覆盖。
export function filesDeactivate() {
    seq += 1;
}

/// 切换节点：每台机器的白名单与路径都不同，回到默认根目录重新开始。
export function filesReset() {
    seq += 1;
    transferTask?.cancel();
    transferTask = null;
    $('files-transfer').hidden = true;
    $('files-resume').hidden = true;
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
    for (const id of ['files-upload', 'files-upload-dir', 'files-mkdir', 'files-touch']) {
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
    // 目录下载在服务端打包成 zip；符号链接的目标可能在白名单之外，不提供下载。
    if (entry.kind !== 'link') actions.push(actionButton('download', entry.is_dir ? '打包' : '下载', false));
    // 归档解压到旁边的同名目录；只读模式与符号链接都不提供入口。
    if (writable && entry.kind !== 'link' && isArchive(entry.name)) {
        actions.push(actionButton('extract', '解压', false));
    }
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

/// 可解压的归档后缀，与服务端支持的范围一致。
const ARCHIVE_SUFFIXES = ['.zip', '.tar', '.tar.gz', '.tgz'];

function isArchive(name) {
    const lower = String(name || '').toLowerCase();
    return ARCHIVE_SUFFIXES.some((suffix) => lower.endsWith(suffix));
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
    if (act === 'extract') extractEntry(entry, button);
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

/// 超过这个字节数改为分片上传，单片大小相同。
const CHUNK = 8 * 1024 * 1024;
/// 分片失败后的重试次数：每次重试都从服务端记录的偏移接着传。
const RETRY = 3;
/// 未完成上传的记录：刷新页面后还能挑同一个文件接着传。
const RESUME_KEY = 'gugu_resume_upload';

/// 当前正在跑的传输；同一时刻只展示一行进度。
let transferTask = null;

async function download(entry) {
    const archive = entry.is_dir;
    const name = archive ? `${entry.name}.zip` : entry.name;
    // 「另存为」必须在用户手势里调用，放在第一次 await 之前。
    const sink = await pickSink(name);
    if (!sink) return;

    const task = beginTransfer(`${archive ? '打包下载' : '下载'} ${entry.name}`);
    const path = archive
        ? `/fs/archive?path=${encodeURIComponent(entry.path)}`
        : `/fs/download?path=${encodeURIComponent(entry.path)}`;
    try {
        const resp = await fetchStream(path, { signal: task.signal });
        const total = Number(resp.headers.get('content-length') || 0);
        const written = await pipe(resp, sink, total, task);
        await sink.close();
        toast(`已下载 ${name}（${fmtBytes(written)}）`, 'success');
    } catch (error) {
        await sink.abort();
        reportTransferError(error, `${archive ? '打包下载' : '下载'}失败`);
    } finally {
        task.close();
    }
}

/// 保存目标：优先系统「另存为」对话框并边下边写盘；不支持时退回内存缓冲。
/// 用户在对话框里取消时返回 null。
async function pickSink(name) {
    if (typeof window.showSaveFilePicker === 'function') {
        try {
            const handle = await window.showSaveFilePicker({ suggestedName: name });
            const writable = await handle.createWritable();
            return {
                write: (chunk) => writable.write(chunk),
                close: () => writable.close(),
                abort: () => writable.abort().catch(() => {}),
            };
        } catch (error) {
            if (error?.name === 'AbortError') return null;
        }
    }
    const chunks = [];
    return {
        write: async (chunk) => {
            chunks.push(chunk);
        },
        close: async () => saveBlob(new Blob(chunks), name),
        abort: async () => {},
    };
}

function saveBlob(blob, name) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 0);
}

/// 逐块读出响应并写进目标，顺带把进度推给传输条。
async function pipe(resp, sink, total, task) {
    if (!resp.body) {
        const blob = await resp.blob();
        await sink.write(blob);
        task.update(blob.size, total || blob.size);
        return blob.size;
    }
    const reader = resp.body.getReader();
    let written = 0;
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        await sink.write(value);
        written += value.length;
        task.update(written, total);
    }
    return written;
}

/// 开一行传输进度。取消只中止请求：分片会话与已收字节留在服务端，重新选同一个文件即可接着传。
function beginTransfer(label) {
    const controller = new AbortController();
    const task = {
        signal: controller.signal,
        update(done, total) {
            const percent = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
            $('transfer-fill').style.width = `${percent}%`;
            $('transfer-meta').textContent = total > 0
                ? `${percent}%（${fmtBytes(done)} / ${fmtBytes(total)}）`
                : fmtBytes(done);
        },
        close() {
            if (transferTask !== task) return;
            transferTask = null;
            $('files-transfer').hidden = true;
        },
        cancel() {
            controller.abort();
        },
    };
    $('files-transfer').hidden = false;
    $('transfer-label').textContent = label;
    $('transfer-meta').textContent = '准备中';
    $('transfer-fill').style.width = '0%';
    transferTask = task;
    return task;
}

function reportTransferError(error, fallback) {
    if (error?.name === 'AbortError') {
        toast('已取消传输', 'info');
        return;
    }
    if (error?.message === 'Unauthorized') return;
    toast(error?.message || fallback, 'error');
}

/// 取消之后：这次上传还留着分片会话就提示可以接着传，否则只是放弃这一轮。
function paused(path) {
    if (readResume()?.path === path) {
        toast('已暂停上传，可重新选择同一个文件继续', 'info');
        showResume();
        return;
    }
    toast('已取消上传', 'info');
}

async function upload(files) {
    if (!files.length) return;
    if (!writable) {
        toast('当前是只读模式，无法上传', 'info');
        return;
    }
    await uploadMany(files.map((file) => ({ dir: current, file })));
    await reload();
}

/// 串行上传一组文件，最后汇总一句提示。
async function uploadMany(items) {
    let uploaded = 0;
    for (const item of items) {
        if (await uploadOne(item.dir, item.file)) uploaded += 1;
    }
    if (uploaded) toast(`已上传 ${uploaded} 个文件`, 'success');
    return uploaded;
}

/// 上传单个文件：同名时确认覆盖，大文件自动转分片。返回是否成功。
async function uploadOne(dir, file) {
    const target = join(dir, file.name);
    try {
        await withTransfer(`上传 ${file.name}`, (task) => sendUpload(target, file, false, task));
        return true;
    } catch (error) {
        if (error?.name === 'AbortError') {
            paused(target);
            return false;
        }
        if (error?.message === 'Unauthorized') return false;
        if (!isExistsError(error)) {
            toast(error.message || `上传 ${file.name} 失败`, 'error');
            return false;
        }
    }

    const ok = await confirmAction({
        title: '覆盖同名文件',
        text: `「${target}」已经存在，确认用上传的内容覆盖它？`,
        okText: '覆盖',
        danger: true,
    });
    if (!ok) return false;
    try {
        await withTransfer(`上传 ${file.name}`, (task) => sendUpload(target, file, true, task));
        return true;
    } catch (error) {
        if (error?.name === 'AbortError') {
            paused(target);
        } else if (error?.message !== 'Unauthorized') toast(error.message || `覆盖 ${file.name} 失败`, 'error');
        return false;
    }
}

async function withTransfer(label, run) {
    const task = beginTransfer(label);
    try {
        return await run(task);
    } finally {
        task.close();
    }
}

async function sendUpload(target, file, overwrite, task) {
    if (file.size > CHUNK) return sendChunked(target, file, overwrite, task);
    const url = `/fs/upload?path=${encodeURIComponent(target)}&overwrite=${overwrite}`;
    task.update(0, file.size);
    return uploadXhr('POST', url, file, {
        signal: task.signal,
        onProgress: (loaded) => task.update(loaded, file.size),
    });
}

/// 分片上传：每片顺序追加，失败从服务端偏移重试，页面刷新后可按同一文件续传。
async function sendChunked(target, file, overwrite, task, session = null) {
    const started = session
        ? await checkSession(session.id, target, file.size)
        : await api('POST', `/fs/upload/session?path=${encodeURIComponent(target)}`, {
            path: target,
            size: file.size,
            overwrite,
        });
    const id = started.id;

    let offset = started.offset || 0;
    let attempts = 0;
    task.update(offset, file.size);
    saveResume({ id, node: state.node, path: target, name: file.name, size: file.size, offset });

    while (offset < file.size) {
        const end = Math.min(offset + CHUNK, file.size);
        try {
            const result = await uploadXhr(
                'PUT',
                `/fs/upload/session/${id}?offset=${offset}&path=${encodeURIComponent(target)}`,
                file.slice(offset, end),
                {
                    signal: task.signal,
                    onProgress: (loaded) => task.update(offset + loaded, file.size),
                },
            );
            offset = result?.offset ?? end;
            attempts = 0;
            saveResume({ id, node: state.node, path: target, name: file.name, size: file.size, offset });
        } catch (error) {
            if (error?.name === 'AbortError' || error?.message === 'Unauthorized') throw error;
            attempts += 1;
            if (attempts > RETRY) throw error;
            // 断线后以服务端为准：上一片可能已经落盘，半片也已被截断。
            offset = await sessionOffset(id, target);
            task.update(offset, file.size);
        }
    }

    const done = await api('POST', `/fs/upload/session/${id}/finish?path=${encodeURIComponent(target)}`);
    clearResume();
    return done;
}

async function sessionOffset(id, target) {
    const view = await api('GET', `/fs/upload/session/${id}?path=${encodeURIComponent(target)}`);
    return view.offset || 0;
}

async function checkSession(id, target, size) {
    const view = await api('GET', `/fs/upload/session/${id}?path=${encodeURIComponent(target)}`);
    if (view.size !== size || view.path !== target) {
        clearResume();
        throw new Error('远端会话与所选文件不一致，请重新上传');
    }
    return view;
}

/// 未完成的上传只记在会话存储里：浏览器不保留文件句柄，续传要用户重新选中。
function saveResume(info) {
    try {
        sessionStorage.setItem(RESUME_KEY, JSON.stringify(info));
    } catch {
        // 隐私模式下写不进去，不影响本次上传。
    }
}

function readResume() {
    try {
        return JSON.parse(sessionStorage.getItem(RESUME_KEY) || 'null');
    } catch {
        return null;
    }
}

function clearResume() {
    try {
        sessionStorage.removeItem(RESUME_KEY);
    } catch {
        // 同上。
    }
    $('files-resume').hidden = true;
}

function showResume() {
    const saved = readResume();
    if (!saved || saved.node !== state.node) {
        $('files-resume').hidden = true;
        return;
    }
    $('files-resume').hidden = false;
    $('resume-label').textContent = `未完成的上传：${saved.name}（${fmtBytes(saved.offset || 0)} / ${fmtBytes(saved.size)}）→ ${saved.path}`;
}

async function resumeUpload(file) {
    const saved = readResume();
    if (!saved || !file) return;
    if (file.size !== saved.size) {
        toast('所选文件与未完成上传的大小不一致，请选同一个文件', 'error');
        return;
    }
    try {
        await withTransfer(`续传 ${file.name}`, (task) => sendChunked(saved.path, file, false, task, saved));
        toast(`已续传 ${file.name}`, 'success');
        clearResume();
        await reload();
    } catch (error) {
        if (error?.name === 'AbortError') paused(saved.path);
        else if (error?.message !== 'Unauthorized') toast(error.message || '续传失败', 'error');
    }
}

async function abortResume() {
    const saved = readResume();
    if (saved) {
        await api('DELETE', `/fs/upload/session/${saved.id}?path=${encodeURIComponent(saved.path)}`).catch(() => {});
    }
    clearResume();
    toast('已放弃这次上传', 'info');
}

/// 「上传目录」：`webkitRelativePath` 里带着顶层目录与相对结构。
async function uploadDirectory(files) {
    if (!writable) {
        toast('当前是只读模式，无法上传', 'info');
        return;
    }
    if (!files.length) return;
    const items = files.map((file) => {
        const rel = file.webkitRelativePath || file.name;
        const parts = rel.split('/');
        const dir = parts.length > 1 ? join(current, parts.slice(0, -1).join('/')) : current;
        return { dir, file };
    });
    // 先按深度建目录再传文件；mkdir 的 parents 语义让重复调用无害。
    for (const dir of [...new Set(items.map((item) => item.dir))].sort()) {
        await api('POST', `/fs/mkdir?path=${encodeURIComponent(dir)}`, { path: dir, parents: true }).catch((error) => {
            if (error.message !== 'Unauthorized') toast(error.message || '创建目录失败', 'error');
        });
    }
    await uploadMany(items);
    await reload();
}

function initDrop() {
    const view = $('view-files');
    let depth = 0;
    const carriesFiles = (event) => Array.from(event.dataTransfer?.types || []).includes('Files');

    view.addEventListener('dragenter', (event) => {
        if (!carriesFiles(event)) return;
        event.preventDefault();
        depth += 1;
        $('files-drop-path').textContent = current || '根目录';
        $('files-drop').hidden = false;
    });
    view.addEventListener('dragover', (event) => {
        if (carriesFiles(event)) event.preventDefault();
    });
    view.addEventListener('dragleave', () => {
        depth = Math.max(0, depth - 1);
        if (!depth) $('files-drop').hidden = true;
    });
    view.addEventListener('drop', (event) => {
        if (!carriesFiles(event)) return;
        event.preventDefault();
        depth = 0;
        $('files-drop').hidden = true;
        dropItems(event.dataTransfer);
    });
}

/// 拖入的可能是若干文件，也可能是整个目录；目录按原结构先建后传。
async function dropItems(data) {
    if (!writable) {
        toast('当前是只读模式，无法上传', 'info');
        return;
    }
    const entries = Array.from(data.items || [])
        .map((item) => (typeof item.webkitGetAsEntry === 'function' ? item.webkitGetAsEntry() : null))
        .filter(Boolean);
    try {
        if (!entries.length) {
            await uploadMany(Array.from(data.files || []).map((file) => ({ dir: current, file })));
        } else {
            for (const entry of entries) {
                if (entry.isFile) await uploadOne(current, await fileOf(entry));
                else if (entry.isDirectory) await uploadEntryDir(entry, current);
            }
        }
    } catch (error) {
        if (error?.message !== 'Unauthorized') toast(error.message || '上传失败', 'error');
    }
    await reload();
}

async function uploadEntryDir(dirEntry, parent) {
    const target = join(parent, dirEntry.name);
    await api('POST', `/fs/mkdir?path=${encodeURIComponent(target)}`, { path: target, parents: true });
    for (const child of await readEntries(dirEntry)) {
        if (child.isFile) {
            await uploadOne(target, await fileOf(child));
        } else if (child.isDirectory) {
            await uploadEntryDir(child, target);
        }
    }
}

/// 目录项要分批读，直到返回空批。
function readEntries(dirEntry) {
    return new Promise((resolve, reject) => {
        const reader = dirEntry.createReader();
        const all = [];
        const next = () => {
            reader.readEntries((batch) => {
                if (!batch.length) {
                    resolve(all);
                    return;
                }
                all.push(...batch);
                next();
            }, reject);
        };
        next();
    });
}

function fileOf(entry) {
    return new Promise((resolve, reject) => entry.file(resolve, reject));
}

/// 上传冲突由服务端按 400 返回，消息里带「已存在」。
function isExistsError(error) {
    return String(error?.message || '').includes('已存在');
}

// ── 解压 ──────────────────────────────────────────────

/// 解压归档到旁边的同名目录；目标已存在时先确认整目录覆盖。
///
/// 进行中把按钮置成「解压中」，比飘一条提示更贴位置，重绘后按钮自然恢复。
async function extractEntry(entry, button) {
    if (!writable) return;
    const url = `/fs/extract?path=${encodeURIComponent(entry.path)}`;
    const busy = (on) => {
        if (!button) return;
        button.disabled = on;
        button.textContent = on ? '解压中' : '解压';
    };
    busy(true);
    try {
        const result = await api('POST', url, { path: entry.path });
        toast(result?.message || `已解压 ${entry.name}`, 'success');
        await reload();
        return;
    } catch (error) {
        if (error?.message === 'Unauthorized') return;
        if (!String(error?.message || '').includes('目标已存在')) {
            toast(error?.message || `解压 ${entry.name} 失败`, 'error');
            return;
        }
    } finally {
        busy(false);
    }

    const ok = await confirmAction({
        title: '覆盖解压目标',
        text: `「${entry.name}」对应的目标目录已经存在，确认删掉它并重新解压？`,
        okText: '覆盖',
        danger: true,
    });
    if (!ok) return;
    busy(true);
    try {
        const result = await api('POST', url, { path: entry.path, overwrite: true });
        toast(result?.message || `已解压 ${entry.name}`, 'success');
        await reload();
    } catch (error) {
        if (error?.message !== 'Unauthorized') toast(error?.message || `解压 ${entry.name} 失败`, 'error');
    } finally {
        busy(false);
    }
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
