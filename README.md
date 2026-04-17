# GuguGuard (咕咕鸽进程守护)

[English](./README_EN.md)

跨平台进程守护与管理工具，使用 Rust 构建。通过 CLI 和 WebUI 管理你的服务进程。

## 功能特性

- **进程管理** — 启动、停止、重启、自动重启、崩溃恢复
- **Web 管理面板** — 暗色主题 UI，动态表单，实时状态更新
- **命令行工具** — 完整的 CLI，支持彩色表格输出
- **日志捕获** — stdout/stderr 自动捕获，内存环形缓冲区 + 文件持久化 + 自动轮转
- **健康检查** — TCP 端口检测 / HTTP 请求检测，支持失败自动重启
- **文件浏览器** — 服务端文件浏览，快速选择工作目录和可执行文件
- **API Key 认证** — 可选的 API Key 保护，WebUI 登录界面
- **CORS 配置** — 可限制允许的跨域来源
- **进程依赖** — `depends_on` 配置启动顺序，支持拓扑排序
- **配置热重载** — SIGHUP (Linux) 或 `POST /api/v1/reload` 重新加载配置
- **智能更新** — 修改非运行时配置（日志路径、重启策略等）不重启进程
- **开机自启** — Windows (任务计划程序) / Linux (systemd)
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

编译后只需一个 `gugu` 可执行文件，WebUI 已内嵌在二进制中，无需额外文件。

### 创建配置

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
depends_on = ["my-db"]           # 等待 my-db 启动后再启动

[processes.my-app.health_check]
type = "tcp"
port = 8080
interval_secs = 30
timeout_secs = 5
# unhealthy_restart = true       # 健康检查失败时自动重启
```

### 启动守护进程

```bash
# 前台运行
gugu run

# 指定配置文件
gugu run -c /path/to/config.toml

# 通过 CLI 参数设置 API Key (优先级高于配置文件)
gugu --api-key "your-secret-key" run
```

打开浏览器访问 `http://localhost:9090` 即可使用 Web 管理面板。

### 开机自启

```bash
# Windows (需管理员权限)
gugu install

# Linux (需 sudo)
sudo gugu install
```

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
gugu run                          # 启动守护进程
gugu status                       # 查看所有进程状态
gugu list                         # 列出已配置进程
gugu start <name>                 # 启动进程
gugu stop <name>                  # 停止进程
gugu restart <name>               # 重启进程
gugu logs <name> [-l 100]         # 查看日志
gugu add <name> -x "node app.js"  # 添加进程
gugu remove <name>                # 移除进程
gugu reload                       # 重新加载配置文件
gugu install                      # 注册开机自启
gugu uninstall                    # 卸载开机自启
```

全局选项：

```bash
-c, --config <PATH>      # 指定配置文件 (默认 gugu.toml)
    --server <URL>       # 指定服务端地址
    --api-key <KEY>      # 指定 API Key (优先级高于配置文件)
```

### 添加进程示例

```bash
gugu add my-api -x "python -m uvicorn main:app --host 0.0.0.0" \
  --dir /home/user/api \
  --env PORT=8000,DEBUG=false \
  --start

gugu add frontend -x "bun run dev" \
  --dir /home/user/web \
  --no-auto-start
```

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
| GET | `/api/v1/processes/:name/logs` | 获取日志 |
| GET | `/api/v1/processes/:name/config` | 获取配置 |
| GET | `/api/v1/stats` | 统计信息 |
| GET | `/api/v1/fs/browse?path=.` | 浏览文件系统 |
| POST | `/api/v1/reload` | 重新加载配置 |
| WS | `/api/v1/ws` | 实时状态推送 |

> 设置 `api_key` 后，所有 API 请求需携带 `Authorization: Bearer <key>` 请求头，WebSocket 连接需添加 `?token=<key>` 查询参数。

## 项目结构

```
gugu-guard/
├── crates/
│   ├── core/       # 核心库: 配置、进程管理、日志、健康检查
│   ├── server/     # Web 服务: REST API、WebSocket、认证、静态文件
│   └── cli/        # 命令行工具
├── web/            # WebUI (HTML/CSS/JS)
├── config.example.toml
└── Cargo.toml      # Workspace 根配置
```

## 配置参考

### 守护进程配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `daemon.pid_file` | string | — | PID 文件路径 |
| `daemon.log_dir` | string | — | 日志目录 |
| `daemon.api_key` | string | — | API Key，设置后启用认证 |
| `daemon.web.addr` | string | `0.0.0.0` | 监听地址 |
| `daemon.web.port` | u16 | `9090` | 监听端口 |
| `daemon.web.cors_origins` | string[] | `[]` | CORS 允许来源 (空则允许所有) |

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
| `stop_timeout_secs` | u64 | `10` | 停止超时（秒） |
| `depends_on` | string[] | `[]` | 依赖的进程名，按依赖顺序启动 |
| `max_log_size_mb` | u64 | — | 日志文件大小上限 (MB)，超限自动轮转 |
| `unhealthy_restart` | bool | `false` | 健康检查失败时自动重启 |
| `stdout_log` | string | — | stdout 日志文件路径 |
| `stderr_log` | string | — | stderr 日志文件路径 |

### 健康检查

```toml
[processes.my-app.health_check]
type = "tcp"              # 或 "http"
port = 8080               # TCP 模式
# url = "http://localhost:8080/health"  # HTTP 模式
interval_secs = 30
timeout_secs = 5
```

## License

MIT
