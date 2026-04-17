# GuguGuard (咕咕鸽进程守护)

[中文文档](./README.md)

A cross-platform process guardian and manager built with Rust. Manage your service processes via CLI and WebUI.

## Features

- **Process Management** — Start, stop, restart, auto-restart, crash recovery
- **Web Dashboard** — Dark-themed UI, dynamic forms, real-time status updates
- **CLI Tool** — Full-featured command line with colored table output
- **Log Capture** — Automatic stdout/stderr capture, in-memory ring buffer + file persistence + auto rotation
- **Health Checks** — TCP port / HTTP request monitoring with optional auto-restart on failure
- **File Browser** — Server-side file browsing for quick directory and executable selection
- **API Key Auth** — Optional API key protection with WebUI login dialog
- **CORS Config** — Configurable allowed cross-origin origins
- **Process Dependencies** — `depends_on` for startup ordering with topological sort
- **Hot Config Reload** — SIGHUP (Linux) or `POST /api/v1/reload` to reload config at runtime
- **Smart Updates** — Non-runtime config changes (log paths, restart policies, etc.) don't restart the process
- **Auto-start on Boot** — Windows (Task Scheduler) / Linux (systemd)
- **Single Instance** — PID file lock to prevent duplicate daemons
- **Graceful Shutdown** — Ctrl+C / SIGTERM / SIGHUP signal handling
- **Cross-platform** — Windows & Linux

## Quick Start

### Install

**Option 1: Build from source**

```bash
git clone https://gitee.com/hongweifei/gugu-guard.git
cd gugu-guard
cargo build --release
# Binary at target/release/gugu
```

**Option 2: cargo install**

```bash
cargo install --git https://gitee.com/hongweifei/gugu-guard.git
```

After compilation, you only need a single `gugu` binary — the WebUI is embedded at compile time, no extra files required.

### Create Config

```bash
cp config.example.toml gugu.toml
```

Edit `gugu.toml`:

```toml
[daemon]
# api_key = "your-secret-key"   # Optional, enables API + WebUI authentication

[daemon.web]
addr = "0.0.0.0"
port = 9090
# cors_origins = ["http://localhost:3000"]  # Optional, restrict CORS origins

[processes.my-app]
command = "node app.js"
working_dir = "/home/user/my-project"
auto_start = true
auto_restart = true
max_restarts = 3
restart_delay_secs = 5
max_log_size_mb = 10             # Auto-rotate logs when exceeding 10MB
stdout_log = "logs/my-app-stdout.log"
stderr_log = "logs/my-app-stderr.log"
depends_on = ["my-db"]           # Wait for my-db to start first

[processes.my-app.health_check]
type = "tcp"
port = 8080
interval_secs = 30
timeout_secs = 5
# unhealthy_restart = true       # Auto-restart on health check failure
```

### Start the Daemon

```bash
# Run in foreground
gugu run

# Specify config file
gugu run -c /path/to/config.toml

# Set API Key via CLI (overrides config file)
gugu --api-key "your-secret-key" run
```

Open `http://localhost:9090` in your browser to access the web dashboard.

### Auto-start on Boot

```bash
# Windows (requires admin)
gugu install

# Linux (requires sudo)
sudo gugu install
```

### Hot Config Reload

```bash
# Linux: send SIGHUP signal
kill -HUP $(cat gugu.pid)

# Any platform: via CLI
gugu reload

# Or call the API directly
curl -X POST http://localhost:9090/api/v1/reload
```

## CLI Usage

```bash
gugu run                          # Start the daemon
gugu status                       # Show process status
gugu list                         # List configured processes
gugu start <name>                 # Start a process
gugu stop <name>                  # Stop a process
gugu restart <name>               # Restart a process
gugu logs <name> [-l 100]         # View logs
gugu add <name> -x "node app.js"  # Add a process
gugu remove <name>                # Remove a process
gugu reload                       # Reload config file
gugu install                      # Register auto-start
gugu uninstall                    # Unregister auto-start
```

Global options:

```bash
-c, --config <PATH>      # Config file path (default: gugu.toml)
    --server <URL>       # Server URL to connect to
    --api-key <KEY>      # API Key (overrides config file)
```

### Add Process Examples

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

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/processes` | List all processes |
| POST | `/api/v1/processes/:name` | Create a process |
| GET | `/api/v1/processes/:name` | Get process info |
| PUT | `/api/v1/processes/:name` | Update a process |
| DELETE | `/api/v1/processes/:name` | Delete a process |
| POST | `/api/v1/processes/:name/start` | Start |
| POST | `/api/v1/processes/:name/stop` | Stop |
| POST | `/api/v1/processes/:name/restart` | Restart |
| GET | `/api/v1/processes/:name/logs` | Get logs |
| GET | `/api/v1/processes/:name/config` | Get config |
| GET | `/api/v1/stats` | Statistics |
| GET | `/api/v1/fs/browse?path=.` | Browse filesystem |
| POST | `/api/v1/reload` | Reload config |
| WS | `/api/v1/ws` | Real-time status |

> When `api_key` is set, all API requests require the `Authorization: Bearer <key>` header. WebSocket connections require `?token=<key>` query parameter.

## Project Structure

```
gugu-guard/
├── crates/
│   ├── core/       # Core lib: config, process management, logging, health checks
│   ├── server/     # Web server: REST API, WebSocket, auth, static files
│   └── cli/        # Command line tool
├── web/            # WebUI (HTML/CSS/JS)
├── config.example.toml
└── Cargo.toml      # Workspace root config
```

## Configuration Reference

### Daemon Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `daemon.pid_file` | string | — | PID file path |
| `daemon.log_dir` | string | — | Log directory |
| `daemon.api_key` | string | — | API Key, enables auth when set |
| `daemon.web.addr` | string | `0.0.0.0` | Listen address |
| `daemon.web.port` | u16 | `9090` | Listen port |
| `daemon.web.cors_origins` | string[] | `[]` | Allowed CORS origins (empty = allow all) |

### Process Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | **required** | Command to execute |
| `args` | string[] | `[]` | Command arguments |
| `working_dir` | string | — | Working directory |
| `env` | map | `{}` | Environment variables |
| `auto_start` | bool | `true` | Auto-start when daemon starts |
| `auto_restart` | bool | `true` | Auto-restart on crash |
| `max_restarts` | u32 | `3` | Max consecutive crash restart attempts |
| `restart_delay_secs` | u64 | `5` | Restart delay in seconds |
| `stop_timeout_secs` | u64 | `10` | Stop timeout in seconds |
| `depends_on` | string[] | `[]` | Process dependencies, start in dependency order |
| `max_log_size_mb` | u64 | — | Log file size limit (MB), auto-rotate when exceeded |
| `unhealthy_restart` | bool | `false` | Auto-restart on health check failure |
| `stdout_log` | string | — | stdout log file path |
| `stderr_log` | string | — | stderr log file path |

### Health Check

```toml
[processes.my-app.health_check]
type = "tcp"              # or "http"
port = 8080               # TCP mode
# url = "http://localhost:8080/health"  # HTTP mode
interval_secs = 30
timeout_secs = 5
```

## License

MIT
