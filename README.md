# GuguGuard (咕咕鸽进程守护)

[English](./README_EN.md)

跨平台进程守护与管理工具，使用 Rust 构建。通过 CLI 和 WebUI 管理你的服务进程。

## 功能特性

- **进程管理** — 启动、停止、重启、自动重启、崩溃恢复
- **Web 管理面板** — 暗色主题 UI，动态表单，实时状态更新
- **命令行工具** — 完整的 CLI，支持彩色表格输出
- **日志捕获** — stdout/stderr 自动捕获，内存环形缓冲区 + 文件持久化 + 自动轮转
- **健康检查** — TCP 端口 / HTTP 请求检测，连续失败达到阈值才判定重启，瞬时抖动不误伤
- **崩溃历史** — 最近 5 次退出（时间、退出码、是否被自动拉起）在卡片与 API 都能看到；每次启动/退出/拉起另外追加到 30 天滚动日志，跨守护重启保留，可算可用率与崩溃频次
- **资源监控** — 按进程树采样 CPU/内存（含 conhost 等子进程），卡片与 `/metrics` 可见；`max_memory_mb` 超限自动重启，防内存泄漏
- **资源硬限（cgroup v2 / Job Object）** — Linux 下每个进程独占一个 cgroup，`cpu_max` / `memory_max` / `pids_max` 由内核执行：内存越界整组 OOM、任务数封顶，启动前生效，退出即清理；Windows 下同一组字段落到进程树的 Job Object（CPU 速率硬顶、提交内存上限、活动进程数上限）
- **指标时序** — 每分钟一桶保留 24 小时，CPU/内存均值与峰值、连接数随 WebUI「指标」面板与 `GET /api/v1/processes/:name/history` 查看
- **端口预检与连接数** — 配置 `listen_ports` 后启动前检查端口是否被别的进程占用（占用时直接失败并报出占用者），运行中统计这些端口上的已建立连接数
- **就绪探针** — `depends_on` 现在等上游真的可用（健康检查通过或端口进入监听）再拉起下游，最多等 `ready_timeout_secs`
- **日志告警** — 命中关键字、或短时间内输出刷屏，按冷却时间推送通知，避免重试风暴刷爆机器人
- **日志结构化与检索** — 每行解析出级别、正文、结构化字段与行内时间（JSON / logfmt / 文本启发式），日志面板带级别徽标与字段胶囊；按级别、关键字、输出流检索落盘的日志文件，返回命中与扫描范围
- **操作审计** — 谁在何时对哪个进程做了什么：写操作（启停、增删改、重载、停止守护）连同结果、来源、客户端与耗时追加到 `logs/audit.jsonl`，保留 90 天；WebUI「操作审计」与 `gugu audit` 可按动作、结果、时间、关键字筛选
- **WebUI 内置终端** — 浏览器里打开一个真实 PTY（Windows ConPTY / Unix openpty）：`cd`、分页器、彩色输出与交互式程序都照常工作，输出实时流式回传。默认关闭，开启后受 API Key 与回环监听两道安全门槛约束；单实例最多 4 个会话，空闲 30 分钟自动关闭，打开与关闭写入操作审计
- **多节点** — 一台中心节点纳管多台跑着 gugu 的机器：`[[nodes]]` 里写上从节点地址与 Key，WebUI 顶栏切换后直接启停进程、看日志、改配置；节点表每 5 秒探测在线状态与延迟，REST 代理与 WebSocket 中继都由中心节点完成，从节点零改动
- **整机带宽** — 所有物理网卡合计的上下行速率，顶栏与指标面板展示，`/metrics` 暴露
- **异常通知** — 退出后不再拉起 / 健康判定不健康 / 被自动拉起 / 内存超限 均可推送到钉钉、企微、飞书自定义机器人或自建 webhook
- **直接执行** — 命令默认不经 shell 直接启动，带空格/特殊字符的参数不会被拆碎或注入；需要 shell 特性时设 `shell = true`
- **文件浏览器** — 服务端文件浏览，快速选择工作目录和可执行文件
- **API Key 认证** — 可选的 API Key 保护，WebUI 登录界面
- **CORS 配置** — 可限制允许的跨域来源
- **进程依赖** — `depends_on` 配置启动顺序，支持拓扑排序
- **配置热重载** — SIGHUP (Linux) 或 `POST /api/v1/reload` 重新加载配置
- **智能更新** — 修改非运行时配置（日志路径、重启策略等）不重启进程
- **绿色部署** — 配置默认跟着程序走，日志/PID 也在程序目录，整个文件夹搬走即用
- **开机自启** — 系统级（Windows 任务计划程序 / Windows 服务 / Linux systemd）与用户级（无需管理员）三种模式，支持 `--dry-run` 预览
- **防多实例** — PID 文件锁
- **优雅退出** — Ctrl+C / SIGTERM / SIGHUP 信号处理
- **跨平台** — Windows & Linux

## 快速开始

### 安装

**方式一：从源码编译**

```bash
git clone https://gitee.com/hongweifei/gugu-guard.git
cd gugu-guard
cargo build --release
# 二进制文件在 target/release/gugu
```

**方式二：cargo install**

```bash
cargo install --git https://gitee.com/hongweifei/gugu-guard.git
```

**方式三：下载预编译绿色包**

到 Releases 下载对应平台的压缩包（`gugu-windows-x86_64.zip` / `gugu-linux-x86_64.tar.gz`），解压即用。
包内只有一个可执行文件加许可与示例配置——**配置、PID、日志都会在首次运行时生成在程序同目录**，
整个文件夹可以随意搬移。

编译后只需一个 `gugu` 可执行文件，WebUI 已内嵌在二进制中，无需额外文件。

### 配置文件（绿色部署）

配置**默认跟着程序走**。启动时按以下顺序寻找，找不到会自动在程序目录生成一份带注释的模板：

1. `-c` 显式指定的路径（优先级最高）
2. **程序同目录**下的 `gugu.toml`（推荐位置）
3. 当前目录下的 `gugu.toml`（方便在源码目录直接运行）

配置里的相对路径（日志、PID、工作目录）都以**配置文件所在目录**为基准，因此整个文件夹可以随程序一起搬走，无需改配置：

```
gugu-guard/
├── gugu.exe        # 单文件即可，WebUI 已内嵌
├── gugu.toml       # 首次运行自动生成在程序旁边
├── gugu.pid
└── logs/           # 进程日志也统一放这里
```

也可以手动复制示例配置再编辑：

```bash
cp config.example.toml gugu.toml
```

编辑 `gugu.toml`：

```toml
[daemon]
# api_key = "your-secret-key"   # 可选，设置后 API 和 WebUI 需要认证

[daemon.web]
addr = "0.0.0.0"
port = 9090
# cors_origins = ["http://localhost:3000"]  # 可选，限制跨域来源

[processes.my-app]
command = "node app.js"
working_dir = "/home/user/my-project"
auto_start = true
auto_restart = true
max_restarts = 3
restart_delay_secs = 5
max_log_size_mb = 10             # 日志文件超过 10MB 自动轮转
stdout_log = "logs/my-app-stdout.log"
stderr_log = "logs/my-app-stderr.log"
listen_ports = [8080]            # 端口预检 + 连接数统计 + 依赖就绪判断
depends_on = ["my-db"]           # 等 my-db 真的可用后再启动

[processes.my-app.health_check]
type = "tcp"
port = 8080
interval_secs = 30
timeout_secs = 5
# unhealthy_restart = true       # 健康检查失败时自动重启
```

### 启动守护进程

```bash
# 前台运行 (自动读取程序同目录的 gugu.toml，首次运行会生成)
gugu run

# 指定配置文件
gugu run -c /path/to/config.toml

# 通过 CLI 参数设置 API Key (优先级高于配置文件)
gugu --api-key "your-secret-key" run
```

打开浏览器访问 `http://localhost:9090` 即可使用 Web 管理面板。

### 开机自启

三种模式，按「要不要管理员 / 要不要未登录也跑」选：

| 模式 | 适用 | 权限 | 行为 |
|---|---|---|---|
| `--mode task`（默认） | Windows / Linux | 需管理员 | Windows 任务计划程序（`onstart`，SYSTEM 身份）/ Linux systemd 系统单元 |
| `--mode service` | 仅 Windows | 需管理员 | Windows 服务（自动启动 + 崩溃阶梯重启 5s/10s/30s） |
| `--mode user` | Windows / Linux | **无需管理员** | 当前用户登录自启：Windows Run 键 / Linux `systemd --user`（配合 `loginctl enable-linger` 可未登录也起） |

```bash
# 系统级（需管理员 / sudo）
gugu install                       # Windows: 任务计划程序 (开机启动)
sudo gugu install                  # Linux: systemd 系统单元 (开机启动)
gugu install --mode service        # Windows: 注册为 Windows 服务

# 用户级（不需要任何特权，登录即启）
gugu install --mode user

# 先预览将要写入/执行的内容，不改动系统
gugu install --mode task --dry-run
gugu uninstall --dry-run
```

安装时会先做**权限预检**（不满足会直接告诉你改用 `sudo`/管理员，或改用 `--mode user`），
安装后**回读真实状态**（`systemctl is-enabled` / `schtasks /query` / `reg query` / `sc query`），
卸载对「本来就没装」保持容错；`--dry-run` 只打印计划，不碰系统。

> 含空格的安装路径也能正常工作：systemd 单元与 Windows 自启命令里的路径都做了引号包裹。

### 配置热重载

```bash
# Linux: 发送 SIGHUP 信号
kill -HUP $(cat gugu.pid)

# 任何平台: 通过 API 触发
gugu reload

# 也可直接调用 API
curl -X POST http://localhost:9090/api/v1/reload
```

## CLI 用法

```bash
gugu run                          # 启动守护进程 (前台)
gugu daemon start                 # 后台启动守护进程 (分离运行，日志进文件)
gugu daemon stop                  # 优雅停止守护进程 (停子进程 → 关服务 → 清 pid)
gugu status                       # 查看所有进程状态
gugu list                         # 列出已配置进程
gugu nodes                        # 列出中心节点纳管的节点与在线状态
gugu --node web1 status           # 操作从节点（经中心节点代理，命令与本地一致）
gugu --node web1 logs api -l 50
gugu start <name>                 # 启动进程
gugu stop <name>                  # 停止进程
gugu restart <name>               # 重启进程
gugu logs <name> [-l 100]         # 查看日志
gugu logs <name> --level warn -q 超时 --stream stderr
                                  # 实时日志，按最低级别 / 关键字 / 输出流过滤
gugu logs <name> --file -q 超时   # 检索落盘的历史日志（默认倒读最近 4MB）
gugu history <name> [-m 60]       # 指标时序（CPU/内存/连接数，每分钟一桶）
gugu exits <name> [-m 60]         # 退出历史与可用率（落盘记录）
gugu net [-m 60]                  # 整机带宽时序
gugu audit [-l 100] [--failed]    # 操作审计（新的在前；--action 按动作码，--since 按分钟）
gugu add <name> -x "node app.js"  # 添加进程
gugu remove <name>                # 移除进程
gugu reload                       # 重新加载配置文件
gugu check                        # 校验配置文件（只读，不启动进程）
gugu status --json                # 以 JSON 输出，便于脚本消费（list 同）
gugu install                      # 注册开机自启 (--mode task|service|user)
gugu install --mode user          # 用户级自启（无需管理员，登录即启）
gugu install --dry-run            # 仅预览将写入/执行的内容
gugu uninstall                    # 卸载开机自启 (同样支持 --mode / --dry-run)
```

全局选项：

```bash
-c, --config <PATH>      # 指定配置文件 (默认：程序同目录的 gugu.toml)
    --server <URL>       # 指定服务端地址
    --api-key <KEY>      # 指定 API Key (优先级高于配置文件)
    --node <NAME>        # 操作哪台节点：gugu nodes 里的标识，缺省为本机 local
```

### 添加进程示例

```bash
gugu add my-api -x "python -m uvicorn main:app --host 0.0.0.0" \
  --dir /home/user/api \
  --env PORT=8000,DEBUG=false \
  --port 8000 \
  --cpu-max 50% --memory-max 512M --pids-max 128 \
  --start

gugu add frontend -x "bun run dev" \
  --dir /home/user/web \
  --no-auto-start
```

`--port` 可重复或逗号分隔；填了端口才会做启动前占用预检并统计连接数。
`--no-port-check` 关掉预检，`--ready-timeout` 调整等依赖就绪的上限（默认 60 秒）。
`--cpu-max` / `--memory-max` / `--pids-max` 是资源硬限（Linux 走 cgroup v2，Windows 走 Job Object，详见[资源硬限](#资源硬限)）。

## REST API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/processes` | 列出所有进程 |
| POST | `/api/v1/processes/:name` | 创建进程 |
| GET | `/api/v1/processes/:name` | 获取进程信息 |
| PUT | `/api/v1/processes/:name` | 更新进程 |
| DELETE | `/api/v1/processes/:name` | 删除进程 |
| POST | `/api/v1/processes/:name/start` | 启动 |
| POST | `/api/v1/processes/:name/stop` | 停止 |
| POST | `/api/v1/processes/:name/restart` | 重启 |
| GET | `/api/v1/processes/:name/logs?lines=100&level=warn&q=关键字&stream=stderr` | 实时日志，支持级别 / 关键字 / 输出流过滤 |
| GET | `/api/v1/processes/:name/logs/search?q=关键字&level=warn&limit=200&budget_mb=4` | 检索落盘的历史日志，返回命中、扫描行数与字节数、是否截断 |
| GET | `/api/v1/processes/:name/config` | 获取配置 |
| GET | `/api/v1/processes/:name/history?minutes=60` | 指标时序（每分钟一桶，最长 24 小时） |
| GET | `/api/v1/processes/:name/exits?minutes=60&limit=50` | 退出事件与可用率统计 |
| GET | `/api/v1/system?minutes=60` | 整机带宽时序与当前速率 |
| GET | `/api/v1/audit?limit=100&action=process.start&ok=false&q=关键字&since_minutes=60` | 操作审计（写操作记录，新的在前） |
| GET | `/api/v1/terminal` | 终端是否启用、shell、工作目录、会话上限与空闲超时 |
| WS | `/api/v1/terminal/ws` | 内置终端会话（JSON 帧：`input` / `resize` / `data` / `exit`） |
| GET | `/api/v1/nodes` | 节点表：本机与从节点的在线状态、延迟、版本、进程数 |
| GET | `/api/v1/node` | 本节点自身信息（中心节点每 5 秒探测从节点的也是这个接口） |
| ANY | `/api/v1/nodes/{node}/...` | 代理到从节点的同名接口（方法、查询串、请求体、下载头都原样转发） |
| WS | `/api/v1/ws?node=web1` | 跟随所选节点的实时推送（中心节点与从节点建中继） |
| GET | `/api/v1/stats` | 统计信息 |
| GET | `/api/v1/fs/browse?path=.` | 浏览文件系统（受 `daemon.web.fs_roots` 限制，越界返回 403） |
| POST | `/api/v1/reload` | 重新加载配置 |
| POST | `/api/v1/shutdown` | 优雅停止守护进程（先停所有子进程） |
| WS | `/api/v1/ws` | 实时状态推送 |

> 设置 `api_key` 后，所有 API 请求需携带 `Authorization: Bearer <key>` 请求头，WebSocket 连接需添加 `?token=<key>` 查询参数。
> 从节点代理路径同样受中心节点自己的认证保护；从节点的 Key 只存在于中心节点配置里，`GET /api/v1/nodes` 不会返回它。

## 项目结构

```
gugu-guard/
├── .github/workflows/  # CI 门禁 (fmt/clippy/test) 与 tag 触发的绿色包发布
├── crates/
│   ├── core/       # 核心库: 配置、进程管理、日志、健康检查、指标与退出历史、套接字探针
│   ├── server/     # Web 服务: REST API、WebSocket、认证、静态文件
│   └── cli/        # 命令行工具
├── web/            # WebUI (无构建步骤，直接内嵌进二进制)
│   ├── index.html
│   ├── style.css
│   └── js/         # core(状态/工具) / render(列表) / history(指标) / logs(日志) / form(表单) / nodes(节点切换) / app(接线与启动)
├── config.example.toml
└── Cargo.toml      # Workspace 根配置
```

## 配置参考

配置里出现结构体不认识的键时，启动日志与 `gugu check` 会指出完整路径，并在同层字段中给一个最接近的拼写建议（如 `memory_maxx（是否想写 memory_max）`）。这类键不阻止启动，但会被忽略，所以 `gugu check` 的结论会标注「N 个未知键被忽略」；`gugu reload` 与 `POST /api/v1/reload` 重载时同样会把新出现的未知键写进日志。

### 守护进程配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `daemon.pid_file` | string | — | PID 文件路径 |
| `daemon.log_dir` | string | — | 日志目录 |
| `daemon.node_name` | string | 主机名 | 本机在节点表里的显示名；节点标识始终是 `local` |
| `daemon.api_key` | string | — | API Key，设置后启用认证 |
| `daemon.webhook_url` | string | — | 异常通知的 webhook 地址（钉钉/企微/飞书机器人或自建端） |
| `daemon.webhook_format` | string | `dingtalk` | 载荷格式：`dingtalk`（钉钉与企微同形）/ `feishu` / `raw` |
| `daemon.cgroup_root` | string | `/sys/fs/cgroup` | Linux 上 cgroup v2 硬限的落地目录；容器或 systemd 委派环境指向有写权限的子树 |
| `daemon.web.addr` | string | `0.0.0.0` | 监听地址 |
| `daemon.web.port` | u16 | `9090` | 监听端口 |
| `daemon.web.cors_origins` | string[] | `[]` | CORS 允许来源 (空则允许所有) |
| `daemon.web.fs_roots` | string[] | `[]` | 文件浏览白名单；为空表示不限制，配了只放行这些根目录及其子目录 |
| `daemon.web.terminal` | bool | `false` | 是否开放 WebUI 内置终端；非回环监听时必须同时配置 `api_key` |

> 守护进程自身的运行日志按天写入 `log_dir` 下的 `gugu-daemon.<日期>.log`（保留最近 30 份），
> 前台、开机自启任务、Windows 服务三种形态都会写文件；仅当 stdout 是终端时才额外打印到控制台。

### 进程配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `command` | string | **必填** | 执行命令 |
| `args` | string[] | `[]` | 命令参数 |
| `working_dir` | string | — | 工作目录 |
| `env` | map | `{}` | 环境变量 |
| `auto_start` | bool | `true` | 守护进程启动时自动启动 |
| `auto_restart` | bool | `true` | 崩溃后自动重启 |
| `max_restarts` | u32 | `3` | 最大连续崩溃重启次数 |
| `restart_delay_secs` | u64 | `5` | 重启间隔（秒） |
| `restart_reset_secs` | u64 | `60` | 进程稳定运行达该秒数后，崩溃重启计数自动归零，守护长期有效 |
| `shell` | bool | `false` | true 时将 command+args 拼接后经 shell 执行（支持管道等）；默认直接启动可执行文件 |
| `stop_timeout_secs` | u64 | `10` | 停止超时（秒） |
| `depends_on` | string[] | `[]` | 依赖的进程名，按依赖顺序启动 |
| `max_memory_mb` | u64 | — | 进程树内存上限，超限按自动重启路径重启（防泄漏） |
| `cpu_max` | string | — | 资源硬限：CPU 上限，`50%` / `0.5`（核数）/ `50000 100000`（quota period）；Linux 走 cgroup 配额，Windows 按整机占比节流 |
| `memory_max` | string | — | 资源硬限：内存上限，`512M` / `2G` / 字节数；Linux 越界整组 OOM，Windows 限制 Job 提交内存 |
| `pids_max` | string | — | 资源硬限：进程/任务数上限，防 fork 炸弹；Windows 只统计进程 |
| `max_log_size_mb` | u64 | — | 日志文件大小上限 (MB)，超限自动轮转 |
| `unhealthy_restart` | bool | `false` | 健康检查失败时自动重启 |
| `listen_ports` | u16[] | `[]` | 该进程监听的端口：启动前占用预检、连接数统计、依赖就绪判断都用它 |
| `port_check` | bool | `true` | 启动前检查 `listen_ports` 是否已被别的进程占用 |
| `ready_timeout_secs` | u64 | `60` | 作为下游依赖被等待时，最多等多久就绪 |
| `log_alert` | table | — | 日志关键字与刷屏告警，见下节 |
| `stdout_log` | string | — | stdout 日志文件路径 |
| `stderr_log` | string | — | stderr 日志文件路径 |
| `log_format` | string | `auto` | 日志行结构化格式：`auto` / `json` / `logfmt` / `text` |
| `encoding` | string | `auto` | 日志文本编码：`auto`（UTF-8 优先、GBK 兜底）/ `utf-8` / `gbk`（`cp936`） |

### 健康检查

```toml
[processes.my-app.health_check]
type = "tcp"              # 或 "http"
port = 8080               # TCP 模式
# url = "http://localhost:8080/health"  # HTTP 模式
interval_secs = 30
timeout_secs = 5
fail_threshold = 3        # 连续失败该次数才判定不健康（配合 unhealthy_restart 才重启）
```

### 端口、就绪与日志告警

```toml
[processes.my-app]
listen_ports = [8080]     # 端口预检、连接数统计、依赖就绪判断都以此为准
port_check = true         # 启动前发现端口被别人占着就直接失败，并报出占用者
ready_timeout_secs = 60   # 作为依赖被上游等待时的上限

[processes.my-app.log_alert]
patterns = ["panic", "exception", "error"]  # 命中任一关键字即告警（小写子串匹配）
window_secs = 10          # 刷屏统计窗口
burst_lines = 200         # 窗口内输出达到该行数即告警，0 表示不启用刷屏检测
cooldown_secs = 300       # 同类告警的静默时长，防止重试风暴刷屏
```

`depends_on` 的下游不再只等上游"进程起来了"，而是等它真的可用：配了健康检查就等检查
通过，否则等 `listen_ports` 全部进入监听。等待超时或被依赖进程中途退出，本轮跳过该下游
并推送一条通知。

> 说明：连接数是这些**监听端口**上已建立的 TCP 连接数，从系统套接字表读出；
> 整机带宽是所有物理网卡合计，不是单进程口径（内核不按进程记录网卡字节）。

### 日志格式与检索

```toml
[processes.my-app]
stdout_log = "logs/my-app-stdout.log"
stderr_log = "logs/my-app-stderr.log"
log_format = "auto"       # auto: 逐行按 JSON → logfmt → 文本 回退；也可写死 json / logfmt / text
encoding = "auto"         # auto: UTF-8 优先、GBK 兜底；也可写死 utf-8 / gbk
```

- **编码口径** — 面板、WebSocket 广播与检索都按 `encoding` 解码：`auto` 先按 UTF-8
  严格解码，失败再按 GBK（中文 Windows 的代码页 936）。`ping`、cmd 内部命令、
  Python 默认的 GBK 输出都因此能正常显示中文；日志文件始终写程序输出的原始字节，
  下载出来对得上原样

- **解析口径** — 每行提取级别（`trace` / `debug` / `info` / `warn` / `error` / `fatal`，数字级别兼容
  syslog 0-7 与 bunyan/pino 10-60）、正文、结构化字段与行内时间。JSON 与 logfmt 的
  `level` / `msg` / `time` 等键会被提升，其余键作为字段展示；嵌套对象按 `a.b.c` 展平
- **ANSI 处理** — 内存缓冲、WebSocket 推送与检索结果里的行会清掉颜色与光标控制符；
  落盘文件保留原始字节，下载导出的日志与进程原样输出对得上
- **实时检索** — `GET /logs` 的 `level` / `q` / `stream` 在内存环形缓冲（每进程每流最近 1000 行）上过滤
- **历史检索** — `GET /logs/search` 从最新的数据往前读，`budget_mb`（默认 4，上限 32）是整次检索的读取总量：
  当前文件读满再翻 `.1`…`.6` 轮转备份；用同一解析器逐行结构化后过滤；
  返回命中条数、扫描行数/字节数，以及是否因预算或条数上限被截断
- **时间不冒充** — 历史行如果解析不出自己的时间戳，就不填采集时间：按时间过滤时这类行会被排除

### 操作审计

```bash
gugu audit                        # 最近 100 条控制面操作，新的在前
gugu audit --failed --since 60    # 最近一小时失败的操作
gugu audit --action process.start --json
```

- **记什么** — 所有会改变状态的写请求：进程增删改、启停重启、健康检查、清空日志、分组启停重启、
  重载配置、停止守护。GET 只读接口不进审计，所以审计面板自己的查询不会自我循环
- **一条记录** — 时间、动作码与中文名、对象、HTTP 方法与路径、状态码、耗时、来源地址、
  客户端（`gugu-cli/<版本>` / WebUI / 其他 User-Agent）、认证结果（`none` / `key` / `denied`），
  失败时附错误原因（截断到 300 字）
- **落盘** — 追加到 `<log_dir>/audit.jsonl`（默认 `logs/audit.jsonl`），保留 90 天：
  每天首次写入时按保留期清理一次，文件超过 8MB 再立即压缩，压缩后仍过大就只留较新的半份。
  进程内存里另留最近 500 条供接口直接读
- **认证失败也记** — 401 记为 `denied`，来源、目标、路径都在，便于排查未授权尝试
- **来源判定** — 优先 `X-Forwarded-For` 的第一跳，其次 `X-Real-IP`，否则用连接的对端地址；
  反向代理后面请让代理补这两个头，否则记到的是代理地址。直连暴露时这两个头可被伪造，只应在可信代理后面使用
- **查询** — `GET /api/v1/audit?limit=100&action=process.start&ok=false&q=ghost&since_minutes=60`：
  `action` 为动作码，`ok=true/false` 只看成功或失败，`q` 匹配对象、路径、来源与原因，
  `since_minutes` 限定时间窗口

### 文件浏览白名单

WebUI 的表单用 `/api/v1/fs/browse` 选工作目录与可执行文件。这条接口默认可以浏览整台机器，
对外提供服务时应限定范围：

```toml
[daemon.web]
fs_roots = ["D:/projects", "/srv/apps"]   # 为空表示不限制
```

- **校验在真实路径上做** — 请求路径先 `canonicalize`，`..` 与符号链接都被解析，再按路径分量与白名单比较；
  `/data` 不会把 `/database` 算作子目录
- **缺省落在第一个根** — 打开浏览弹窗时没有起始目录，服务端直接给 `fs_roots[0]`，避免默认工作目录不在白名单里
- **越界返回 403** — 错误信息带上被拒绝的路径，一眼能分清是白名单挡住还是目录不存在
- **未配置的告警** — 监听非回环地址且 `fs_roots` 为空时，启动日志会提示这条能力的暴露面
- **体检** — `gugu check` 打印每个根目录是否存在；空条目直接算配置错误

### WebUI 内置终端

```toml
[daemon]
api_key = "your-secret-key"      # 监听 0.0.0.0 / 局域网地址时必填，否则拒绝启动

[daemon.web]
terminal = true                  # 默认 false；开启后浏览器里就是一个真实 shell
addr = "127.0.0.1"               # 只给本机用时，回环监听可以不带 API Key
```

- **真 PTY，不是逐行执行器** — Windows 走 ConPTY、Unix 走 `openpty`（`portable-pty`）：shell 认为自己在终端里，
  `cd`、`less`、彩色输出、Tab 补全与需要 TTY 的程序都能正常工作；输出按块实时推给浏览器，xterm.js 负责渲染
- **安全门槛** — 终端默认关闭。监听地址是 `127.0.0.1` / `localhost` / `::1` 时可以只靠本机隔离；
  其余地址（含 `0.0.0.0`）必须配置 `daemon.api_key`（或 `GUGU_API_KEY` / `--api-key`），
  否则守护进程在启动阶段直接报错退出。校验放在 manager 启动之前，避免出现「面板能开、终端打不开」的半可用状态
- **认证与同源** — 终端路由挂在受保护路由下，继承 API Key 认证；浏览器 WebSocket 用 `?token=` 传 Key。
  WebSocket 不受浏览器同源策略约束，所以握手还要求 `Origin` 与 `Host` 一致；没有 Key 时 `Host` 必须是回环地址，
  挡住把恶意域名解析到 127.0.0.1 的 DNS rebinding。未启用时 `GET /api/v1/terminal` 返回 `{"enabled": false}`，WS 在升级前直接 403
- **会话资源** — 单实例最多 4 个并发会话，第 5 个返回 429；无输入也无输出 30 分钟后自动结束并回收 PTY；
  关闭面板、断网或点击「关闭」都会 kill 子进程并释放会话名额
- **审计** — 打开写 `terminal.open`（状态 101），关闭写 `terminal.close`（状态 200，detail 记时长与 shell），
  User-Agent 记浏览器；来源优先 `X-Forwarded-For` / `X-Real-IP`，WS 升级拿不到对端地址时记 `-`。
  GET 升级请求不会被审计中间件拦到，由终端 handler 自己记录
- **工作目录** — 新会话落在配置文件所在目录，配置里的相对路径可以直接 `cd logs` 查看
- **前端资源** — xterm.js 5.3.0 与 fit addon 0.8.0 已 vendor 到 `web/vendor/`（MIT，许可证同目录），
  随二进制嵌入，不访问 CDN；升级版本后需要重新编译程序
- **协议** — 客户端 `{"type":"input","data":"..."}`、`{"type":"resize","cols":120,"rows":30}`；
  服务端 `{"type":"ready","shell","cwd","max_sessions","idle_timeout_secs"}`、`{"type":"data","data":"..."}`、
  `{"type":"exit","code","reason"}`

### 多节点

```toml
[daemon]
node_name = "中心机"             # 界面上显示的名字，缺省用主机名

[[nodes]]
name = "web1"                   # 节点标识：URL 路径与 --node 都用它，不能是 . 或 ..
label = "上海 Web"              # 显示名，可省略
url = "http://10.0.0.12:9090"   # 从节点地址
api_key = "从节点的 daemon.api_key"   # 指向非回环地址时必填
```

- **拓扑** — 只有中心节点需要这段配置。从节点照常跑 gugu，不需要知道中心节点存在；它只要开放自己的 HTTP 端口，按需配置 `daemon.api_key`
- **切换即操作** — WebUI 顶栏的节点选择器切换后，进程列表、日志、指标、审计、表单全部指向该节点，启停与改配置直接生效；实时状态与日志通过 `WS /api/v1/ws?node=` 由中心节点中继，仍是 2 秒一推
- **在线状态** — 中心节点每 5 秒探测一次各从节点的 `GET /api/v1/node`，记录在线/离线、往返延迟、版本与进程数；顶栏圆点与 `gugu nodes` 都读这份结果，离线时页面顶部显示最后一条错误
- **HTTP 代理** — `/api/v1/nodes/{node}/...` 把方法、查询串、请求体、`Content-Type` 与下载头原样转发，认证换成从节点自己的 Key；响应边收边转，下载大日志不会在中心节点攒成一份内存副本
- **来源链路** — 转发时带上 `X-Forwarded-For`：请求自带的代理链原样保留，中心节点把直连对端追加在末尾；WebSocket 中继的升级请求同样带这个头。从节点的审计与终端会话记的是原始客户端，不是中心节点
- **命令行** — `gugu nodes` 列出节点；`--node <name>` 让客户端命令走中心节点代理，例如 `gugu --node web1 restart api`。标识按 URL 路径段转义，`.`、`..` 这类会被 URL 折叠成目录的写法在发请求前直接报错，不会落到本机接口上
- **审计** — 代理的写操作在中心节点记一条，动作码与本机一致，对象带上节点名（`web1/api`），本机与从节点的操作不会混在一起；从节点自己的审计里，来源地址取自中心节点转发的 `X-Forwarded-For` 第一跳，客户端类型（CLI / WebUI）按原请求头保留
- **安全** — 指向非回环地址的节点必须配置 `api_key`，否则守护进程拒绝启动（与终端同一道门槛）；从节点的 Key 只出现在中心节点的配置与转发请求里
- **终端** — 内置终端操作的是跑着 gugu 的那台机器，只对本机开放；选中从节点时入口隐藏，要在从节点上开终端就访问它自己的面板
- **生效时机** — 节点表在守护进程启动时读取；改动 `[[nodes]]` 后重启守护进程（`gugu reload` 只重载进程配置与守护参数，不重建节点表）

### 资源硬限

```toml
[daemon]
cgroup_root = "/sys/fs/cgroup"   # 仅 Linux；容器或 systemd 委派环境指向有写权限的子树

[processes.my-app]
cpu_max = "50%"                  # 单核的一半；也可写 0.5（核数）或 "50000 100000"
memory_max = "512M"              # 越过即被硬限拦住，不是「超限重启」
pids_max = "128"                 # 进程/任务数上限，防 fork 炸弹
```

Linux 用 cgroup v2，Windows 用 Job Object；两套机制都在启动阶段落地，配了就必须生效，建不出来直接失败。

- **Linux 前提** — cgroup v2 统一层次（挂载点下有 `cgroup.controllers`），守护进程有写权限：
  以 root 运行，或 `cgroup_root` 指向 systemd 委派给自己的子树。cpu / memory / pids 控制器缺失时启动直接失败并说明缺哪个
- **Linux 生效时机** — 启动前建好 `<cgroup_root>/gugu/<进程名>-<哈希>`、写入限值，子进程在 `fork` 之后、`exec` 之前
  把自己写进 `cgroup.procs`：进程树从第一条指令起就在限值内，没有「先跑起来再搬进去」的空窗
- **Linux 继承与清理** — 子进程、孙进程都落在同一 cgroup；进程退出或被停止时对整个 cgroup 执行 `cgroup.kill` 再删目录，
  不留脱管的孤儿。上次运行留下的同名目录会在下次启动时先清空
- **Linux 内存语义** — `memory.max` 是内核硬限：越过即触发该 cgroup 的 OOM，整组进程一起被杀（`memory.oom.group = 1`），
  同时 `memory.swap.max = 0`，避免靠换页绕过上限。守护进程只会看到进程退出（通常是 137），配了 `auto_restart` 会被拉起
- **Windows 映射** — `memory_max` 落到 Job 的提交内存上限（`JobMemoryLimit`），`pids_max` 落到活动进程数（`ActiveProcessLimit`），
  `cpu_max` 落到 CPU 速率硬顶（`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`）。限值在创建 Job 时写入，主进程 `spawn` 后立即分配进 Job，
  之后派生的子进程自动继承
- **Windows 口径** — CPU 是节流而不是杀进程，`CpuRate` 按整机占比计量：`cpu_max = "10%"` 在 16 逻辑核机器上折算为整机 0.625%，
  启动日志会打印实际占比；`memory_max` 是提交内存（Job 内合计），越界时新的分配失败，进程通常自行报错退出；
  `pids_max` 只统计进程，不统计线程
- **与 `max_memory_mb` 的区别** — `max_memory_mb` 是守护进程的看护逻辑：采样超限后走自动重启；`memory_max` 由操作系统执行，
  进程在被杀之前不可能超用。两者可以同时配
- **其它平台** — 既不是 Linux 也不是 Windows 时，配置任一硬限会在启动时明确报错，不会静默忽略

## 持续集成与发布

- **CI**（`.github/workflows/ci.yml`）：Windows 与 Linux 双平台执行 `cargo fmt --check`、
  `cargo clippy -- -D warnings`、`cargo test`；master 分支额外把分支与标签镜像到 Gitee
  （需在仓库 Secrets 配置 `GITEE_TOKEN`，未配置时该步骤自动跳过）
- **发布**（`.github/workflows/release.yml`）：推送 `v*` 标签即构建两平台绿色包并创建 Release，
  附上 `gugu-windows-x86_64.zip` / `gugu-linux-x86_64.tar.gz`
- 本地等价的校验命令（与 CI 一致）：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 更新日志

版本变更记录见 [CHANGELOG.md](CHANGELOG.md)。

## License

MIT — 见 [LICENSE](./LICENSE)
