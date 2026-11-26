# 更新日志

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)，日期为发布日。

## [Unreleased]

服务可观测性一批，不追求向后兼容：配置新增字段、API 新增接口、结构体新增字段。

### 新增

- **指标时序** —— 每分钟一个桶、保留 24 小时，记录 CPU 均值/峰值、内存均值/峰值与连接数峰值。只在内存里，守护重启后重新累积
- **退出日志落盘** —— 每次启动、退出、自动拉起追加一行 JSONL，按 30 天保留期滚动压缩；文件超过 4MB 时触发一次清理。守护重启后仍能回放，算出窗口内退出次数、崩溃次数（操作者主动停止单独计数、不算崩溃）、累计运行时长与可用率
- **端口预检** —— `listen_ports` 配置后，启动前读系统套接字表检查端口占用，冲突时直接失败并报出占用的进程与 PID
- **连接数统计** —— 监控循环每轮统计这些监听端口上的已建立 TCP 连接，卡片、指标面板与 `/metrics` 可见
- **就绪探针** —— `depends_on` 的下游改为等上游真的可用再拉起：配了健康检查等检查通过，否则等端口进入监听，轮询间隔 500ms，上限 `ready_timeout_secs`
- **日志告警** —— `log_alert` 支持关键字命中（小写子串匹配，每个 pattern 独立冷却）与窗口内输出刷屏（`burst_lines` / `window_secs`），触发后走 webhook 通知
- **整机带宽** —— 所有物理网卡合计的上下行速率（Linux 读 `/proc/net/dev`，Windows 走 `GetIfTable2`，均跳过回环），顶栏与指标面板展示

### 接口与工具

- REST：`GET /api/v1/processes/:name/history`、`GET /api/v1/processes/:name/exits`、`GET /api/v1/system`；WebSocket 的 status 消息带 `system` 字段
- `/metrics` 新增 `gugu_process_connections`、`gugu_system_net_rx_bytes_per_sec`、`gugu_system_net_tx_bytes_per_sec`
- CLI：`gugu history` / `gugu exits` / `gugu net`，`gugu status` 增加端口与连接列；`gugu add` 支持 `--port`、`--no-port-check`、`--ready-timeout`
- WebUI：进程卡片显示端口与连接数，新增「指标」面板（CPU / 内存 / 连接数折线、退出历史、整机带宽），顶栏显示整机上下行；表单暴露端口、预检开关、就绪超时与日志告警

### 行为变更

- `depends_on` 的语义从「上游进程已启动」改为「上游已就绪」；超时或被依赖进程中途退出时，本轮跳过该下游并推送一条通知
- 进程配置新增 `listen_ports`、`port_check`（默认 `true`）、`ready_timeout_secs`（默认 60）、`log_alert`

### 已知边界

- 连接数与带宽都不是单进程口径：连接数只覆盖配置的监听端口，带宽是整机合计。内核不按进程记录网卡字节，单进程精确流量需要 nftables cgroup 计数（root）或 eBPF，本版有意不做

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
