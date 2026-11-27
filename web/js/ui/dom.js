// DOM 小工具：查询、创建、清空、事件绑定。不引入框架，只用原生 API。

export function $(id) {
    return document.getElementById(id);
}

export function el(tag, props = {}, children = []) {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(props)) {
        if (value == null || value === false) continue;
        if (key === 'class') node.className = value;
        else if (key === 'text') node.textContent = value;
        else if (key === 'html') node.innerHTML = value;
        else if (key === 'dataset') Object.assign(node.dataset, value);
        else if (key.startsWith('on') && typeof value === 'function') {
            node.addEventListener(key.slice(2), value);
        } else node.setAttribute(key, value);
    }
    for (const child of Array.isArray(children) ? children : [children]) {
        if (child != null) node.append(child);
    }
    return node;
}

export function clear(node) {
    if (node) node.replaceChildren();
}

export function show(node, visible = true) {
    if (node) node.hidden = !visible;
}

export function toggleClass(node, name, on) {
    if (node) node.classList.toggle(name, !!on);
}

/// 按钮进入「执行中」：禁用并显示转圈，避免重复提交。
export function busy(button, on = true) {
    if (!button) return;
    button.classList.toggle('loading', on);
    button.disabled = on;
}

export function debounce(fn, wait) {
    let timer = null;
    return (...args) => {
        clearTimeout(timer);
        timer = setTimeout(() => fn(...args), wait);
    };
}
