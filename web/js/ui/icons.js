// 内联图标：行内按钮复用同一份 20×20 描边图形，避免各处重复写 SVG。

const PATHS = {
    play: '<path d="M7 5.5l7 4.5-7 4.5z" fill="currentColor" stroke="none"/>',
    stop: '<rect x="6" y="6" width="8" height="8" rx="1.5" fill="currentColor" stroke="none"/>',
    restart: '<path d="M4 10a6 6 0 0110.2-4.2M16 4v3.5h-3.5M16 10a6 6 0 01-10.2 4.2M4 16v-3.5h3.5"/>',
    logs: '<path d="M4 5h12M4 10h12M4 15h8"/>',
    chart: '<path d="M3 16V9M8 16V5M13 16v-4M18 16V7"/>',
    edit: '<path d="M13.5 4.5l2 2L7 15H5v-2l8.5-8.5z"/>',
    trash: '<path d="M5 7h10M8 7V5h4v2M6.5 7l.7 8h5.6l.7-8"/>',
    more: '<circle cx="5" cy="10" r="1.4" fill="currentColor" stroke="none"/><circle cx="10" cy="10" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="10" r="1.4" fill="currentColor" stroke="none"/>',
    health: '<path d="M3 10h3l2-4 3 8 2-4h4"/>',
    chevron: '<path d="M7 5l6 5-6 5"/>',
};

/// 生成 20×20 描边图标；`stroke="none"` 的路径自带填充。
export function icon(name, size = 16) {
    const path = PATHS[name] || '';
    return `<svg viewBox="0 0 20 20" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;
}
