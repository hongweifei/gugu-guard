// 浮层栈：记录当前打开的层，Esc 只关最上面那一层，
// 关闭后把焦点还给触发它的元素。

const stack = [];

/// 打开一层：close 负责该层自己的关闭动作（含出栈）。
/// 同一层重复打开时只更新回调，不重复入栈。
export function pushOverlay(id, close) {
    const existing = stack.find((item) => item.id === id);
    if (existing) {
        existing.close = close;
        return;
    }
    stack.push({
        id,
        close,
        returnFocus: document.activeElement instanceof HTMLElement ? document.activeElement : null,
    });
}

export function popOverlay(id) {
    const index = stack.findIndex((item) => item.id === id);
    if (index === -1) return;
    const [layer] = stack.splice(index, 1);
    if (layer.returnFocus && document.contains(layer.returnFocus)) {
        layer.returnFocus.focus({ preventScroll: true });
    }
}

export function isOpen(id) {
    return stack.some((item) => item.id === id);
}

export function topOverlay() {
    return stack.length ? stack[stack.length - 1].id : null;
}

document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    const top = stack[stack.length - 1];
    if (!top) return;
    event.preventDefault();
    event.stopPropagation();
    top.close();
});
