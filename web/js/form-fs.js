// 文件系统浏览器：目录列举、选中与确认回填，服务于「选择可执行文件」「选择工作目录」。

let fsCallback = null;
let fsCurrentPath = '';
let fsCurrentParent = null;
let fsSelectedItem = null;

let fsListEl = null;
let fsActiveEl = null;

async function fsBrowse(path) {
    fsSelectedItem = null;
    fsActiveEl = null;
    if (!fsListEl) fsListEl = document.getElementById('fs-list');
    try {
        const data = await api('GET', `/fs/browse?path=${encodeURIComponent(path)}`);
        fsCurrentPath = data.path || path;
        fsCurrentParent = data.parent || null;
        document.getElementById('fs-path').value = fsCurrentPath;
        if (!data.entries?.length) {
            fsListEl.innerHTML = '<div class="fs-empty">空目录</div>';
            return;
        }
        const parts = [];
        for (let i = 0, len = data.entries.length; i < len; i++) {
            const e = data.entries[i];
            parts.push('<div class="fs-item ', e.is_dir ? 'fs-dir' : 'fs-file', '" data-i="', i, '" title="', esc(e.path), '"><span>', esc(e.name), '</span></div>');
        }
        fsListEl.innerHTML = parts.join('');
        fsListEl._entries = data.entries;
    } catch { toast('无法浏览目录', 'error'); }
}

document.getElementById('fs-list').addEventListener('click', (e) => {
    const item = e.target.closest('.fs-item');
    if (!item) return;
    if (fsActiveEl) fsActiveEl.classList.remove('active');
    item.classList.add('active');
    fsActiveEl = item;
    const entries = item.parentElement._entries;
    const entry = entries[item.dataset.i];
    fsSelectedItem = { path: entry.path, isDir: entry.is_dir };
});

document.getElementById('fs-list').addEventListener('dblclick', (e) => {
    const item = e.target.closest('.fs-item');
    if (!item) return;
    const entries = item.parentElement._entries;
    const entry = entries[item.dataset.i];
    if (entry.is_dir) {
        fsBrowse(entry.path);
    } else {
        fsSelectedItem = { path: entry.path, isDir: false };
        closeFsBrowser();
    }
});

let fsSelectDir = false;

function openFsBrowser(title, confirmText, selectDir, callback) {
    fsCallback = callback;
    fsSelectDir = selectDir;
    fsSelectedItem = null;
    document.getElementById('fs-title').textContent = title;
    document.getElementById('fs-confirm').textContent = confirmText;
    document.getElementById('fs-backdrop').classList.add('open');
    // 起点为空时交给服务端决定：配了 fs_roots 就落在第一个根，否则用工作目录。
    const startPath = document.getElementById('f-dir').value.trim();
    fsBrowse(startPath);
}

/// 关闭文件浏览器。apply=false 时不把选择回填给调用方（切换节点时用）。
function closeFsBrowser(apply = true) {
    document.getElementById('fs-backdrop').classList.remove('open');
    if (apply && fsCallback) {
        const path = fsSelectedItem ? fsSelectedItem.path : (fsSelectDir ? fsCurrentPath : null);
        if (path) fsCallback(path);
    }
    fsCallback = null;
    fsSelectedItem = null;
    fsActiveEl = null;
    fsSelectDir = false;
}

document.getElementById('btn-pick-cmd').onclick = () => openFsBrowser('选择可执行文件', '选择文件', false, (path) => {
    document.getElementById('f-command').value = path;
    updatePreview();
});
document.getElementById('btn-pick-dir').onclick = () => openFsBrowser('选择工作目录', '选择此目录', true, (path) => {
    document.getElementById('f-dir').value = path;
});
document.getElementById('btn-close-fs').onclick = () => closeFsBrowser(false);
document.getElementById('fs-cancel').onclick = () => closeFsBrowser(false);
document.getElementById('fs-confirm').onclick = closeFsBrowser;
document.getElementById('fs-go').onclick = () => fsBrowse(document.getElementById('fs-path').value.trim());
document.getElementById('fs-up').onclick = () => {
    if (fsCurrentParent) fsBrowse(fsCurrentParent);
};
document.getElementById('fs-path').onkeydown = (e) => { if (e.key === 'Enter') fsBrowse(e.target.value.trim()); };
