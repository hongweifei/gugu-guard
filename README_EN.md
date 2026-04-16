# GuguGuard (咕咕鸽进程守护)

[中文文档](./README.md)

A cross-platform process guardian and manager built with Rust. Manage your service processes via CLI and WebUI.

## Features

- **Process Management** — Start, stop, restart, auto-restart, crash recovery
- **Web Dashboard** — Dark-themed UI, dynamic forms, real-time status updates
- **CLI Tool** — Full-featured command line with colored table output
- **Log Capture** — Automatic stdout/stderr capture, in-memory ring buffer + file persistence
- **Health Checks** — TCP port / HTTP request monitoring
- **File Browser** — Server-side file browsing for quick directory and executable selection
- **Auto-start on Boot** — Windows (Task Scheduler) / Linux (systemd)
- **Single Instance** — PID file lock to prevent duplicate daemons
- **Graceful Shutdown** — Ctrl+C / SIGTERM / SIGHUP signal handling
- **Cross-platform** — Windows & Linux

## Quick Start

### Install

**Option 1: Build from source**

```bash
git clone https://github.com/hongweifei/gugu-guard.git
cd gugu-guard
cargo build --release
# Binary at target/release/gugu
```

**Option 2: cargo install**

```bash
cargo install --git https://github.com/hongweifei/gugu-guard.git
```

After compilation, you only need a single `gugu` binary — the WebUI is embedded at compile time, no extra files required.

### Create Config

```bash
cp config.example.toml gugu.toml
```

Edit `gugu.toml`:

```toml
[daemon.web]
addr = "0.0.0.0"
port = 9090

[processes.my-app]
command = "node app.js"
working_dir = "/home/user/my-project"
auto_start = true
auto_restart = true
max_restarts = 3
restart_delay_secs = 5
stdout_log = "logs/my-app-stdout.log"
stderr_log = "logs/my-app-stderr.log"
```

### Start the Daemon

```bash
# Run in foreground
gugu run

# Specify config file
gugu run -c /path/to/config.toml
```

Open `http://localhost:9090` in your browser to access the web dashboard.

### Auto-start on Boot

```bash
# Windows (requires admin)
gugu install

# Linux (requires sudo)
sudo gugu install
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
gugu install                      # Register auto-start
gugu uninstall                    # Unregister auto-start
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
| GET | `/api/v1/fs/browse?path=.` | Browse filesystem |
| WS | `/api/v1/ws` | Real-time status |

## Project Structure

```
gugu-guard/
├── crates/
│   ├── core/       # Core lib: config, process management, logging, health checks
│   ├── server/     # Web server: REST API, WebSocket, static files
│   └── cli/        # Command line tool
├── web/            # WebUI (HTML/CSS/JS)
├── config.example.toml
└── Cargo.toml      # Workspace root config
```

## Configuration Reference

### Process Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | **required** | Command to execute |
| `args` | string[] | `[]` | Command arguments |
| `working_dir` | string | — | Working directory |
| `env` | map | `{}` | Environment variables |
| `auto_start` | bool | `true` | Auto-start when daemon starts |
| `auto_restart` | bool | `true` | Auto-restart on crash |
| `max_restarts` | u32 | `3` | Max restart attempts |
| `restart_delay_secs` | u64 | `5` | Restart delay in seconds |
| `stop_timeout_secs` | u64 | `10` | Stop timeout in seconds |
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
