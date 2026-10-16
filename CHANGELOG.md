# 更新日志

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)，日期为发布日。

## [0.1.0] - 2026-10-16

首个正式版本：跨平台进程守护与管理工具。单个可执行文件、内置 WebUI，配置 / PID / 日志都跟着程序走。

### 核心能力

- **进程守护** —— 直接启动（不经 shell，参数不被拆解或注入）或 `shell = true` 拼接执行；崩溃自动重启，带次数上限，并在稳定运行 `restart_reset_secs` 后把计数归零
- **依赖顺序** —— `depends_on` 拓扑排序，依赖先起；配置热重载（`SIGHUP` 或 `POST /api/v1/reload`）
- **健康检查** —— TCP / HTTP，连续失败达 `fail_threshold` 判定不健康，可选自动重启
- **崩溃历史** —— 每个进程保留最近 5 次退出（时间、退出码、是否被自动拉起）
- **资源监控** —— 按**进程树**采样 CPU 与内存（含 conhost 这类子进程），`max_memory_mb` 超限按自动重启路径处理
- **异常通知** —— 钉钉 / 企微 / 飞书自定义机器人或自建 webhook：退出后不再拉起、判定不健康、被自动拉起、内存超限
- **日志** —— 按进程捕获 stdout/stderr 并按大小轮转；守护自身日志按天轮转
- **认证** —— 可选 `daemon.api_key`，HTTP 与 WebSocket 同时生效

### 接口与工具

- **WebUI**（编译期内嵌）—— 进程卡片（状态/PID/运行时间/命令/上次退出/资源）、实时日志、增删改、分组批量操作、文件浏览
- **REST API + WebSocket** —— 实时状态与日志推送；`/metrics` 输出 Prometheus 文本格式
- **CLI** —— `run` / `status` / `list` / `start` / `stop` / `restart` / `logs` / `add` / `remove` / `reload` / `check` / `daemon start|stop`；`status`、`list` 支持 `--json`
- **开机自启** —— 系统级（Windows 任务计划 / Windows 服务 / Linux systemd）与用户级（无需管理员）三种模式，支持 `--dry-run` 预览、权限预检、安装后状态回读
- **绿色部署** —— 配置查找优先级：显式 `-c` > 程序同目录 > 当前目录 > 首次运行自动生成模板

### 交付

- 预编译绿色包：`gugu-windows-x86_64.zip` / `gugu-linux-x86_64.tar.gz`，内含单个可执行文件 + `LICENSE` + `README` + `config.example.toml`
- CI：Windows 与 Linux 双平台 `cargo fmt --check` / `clippy -D warnings` / `cargo test` 门禁

### 已知边界

- 配置里**未知键会被静默保留**（不做 `deny_unknown_fields`），表名拼错（如 `[processss.x]`）不会报错
- 默认监听 `0.0.0.0:9090` 且 `/api/v1/fs/browse` 无路径白名单 —— **暴露到公网前请设置 `daemon.api_key` 并限制网络访问**
- 0.x 期间配置文件名、CLI 参数与 API 形状仍可能调整

[0.1.0]: https://github.com/hongweifei/gugu-guard/releases/tag/v0.1.0
