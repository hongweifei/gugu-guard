# 第三方前端资源

这些文件随二进制嵌入（`rust-embed` 收集 `web/`），所以升级版本后必须重新构建程序。

| 文件 | 来源 | 版本 | 许可证 |
| --- | --- | --- | --- |
| `xterm.js` / `xterm.css` | <https://www.npmjs.com/package/xterm> | 5.3.0 | MIT（`LICENSE-xterm.txt`） |
| `addon-fit.js` | <https://www.npmjs.com/package/xterm-addon-fit> | 0.8.0 | MIT（`LICENSE-addon-fit.txt`） |

`xterm.js` 提供终端渲染与输入，`addon-fit.js` 根据容器尺寸计算行列数。
两者都是 UMD 构建，直接以 `<script>` 引入，暴露全局 `Terminal` 与 `FitAddon.FitAddon`；
资源只在浏览器本地执行，不从 CDN 加载。
