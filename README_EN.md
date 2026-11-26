# GuguGuard (咕咕鸽进程守护)

[中文文档](./README.md)

A cross-platform process guardian and manager built with Rust. Manage your service processes via CLI and WebUI.

## Features

- **Process Management** — Start, stop, restart, auto-restart, crash recovery
- **Web Dashboard** — Dark-themed UI, dynamic forms, real-time status updates
- **CLI Tool** — Full-featured command line with colored table output
- **Log Capture** — Automatic stdout/stderr capture, in-memory ring buffer + file persistence + auto rotation
- **Health Checks** — TCP port / HTTP request monitoring; restarts only after N consecutive failures (default 3), so transient blips don't kill healthy processes
- **Exit History** — the last 5 exits (time, exit code, whether it was auto-restarted) are visible on the card and via the API; every start/exit/revival is also appended to a 30-day rolling journal that survives daemon restarts, so availability and crash frequency stay measurable
- **Resource Monitoring** — samples CPU/memory for the whole process tree (including helpers like conhost), shown on the card and in `/metrics`; `max_memory_mb` restarts a process that exceeds its memory cap
- **Hard Limits (cgroup v2)** — on Linux every process gets its own cgroup; `cpu_max` / `memory_max` / `pids_max` are enforced by the kernel (memory overrun OOMs the whole group, task count is capped), applied before the first instruction and cleaned up on exit
- **Metric Time Series** — one bucket per minute kept for 24 hours (CPU/memory average and peak, connection count), read through the WebUI "metrics" panel and `GET /api/v1/processes/:name/history`
- **Port Preflight & Connections** — with `listen_ports` configured, startup fails loudly when the port is already taken (naming the owner), and established connections on those ports are counted while running
- **Readiness Probe** — `depends_on` now waits for the upstream to actually serve (health check passing or ports listening), up to `ready_timeout_secs`
- **Log Alerts** — keyword hits and short bursts of output raise notifications with a cooldown, so a retry storm can't flood the bot
- **Structured Logs & Search** — each line is parsed into level, message, structured fields and an in-line timestamp (JSON / logfmt / text heuristics), so the log panel shows level badges and field chips; logs on disk are searchable by level, keyword and stream, with hit and scan-range counts
- **Operation Audit** — who did what to which process and when: write operations (start/stop, add/update/delete, reload, daemon shutdown) are appended to `logs/audit.jsonl` with result, source, client and duration, kept for 90 days; the WebUI "Operation Audit" panel and `gugu audit` filter by action, result, time and keyword
- **Built-in WebUI Terminal** — a real PTY in the browser (ConPTY on Windows, openpty on Unix): `cd`, pagers, colors and TUI programs behave normally, and output streams back live. Off by default; when enabled it is gated by API Key auth and loopback binding. Up to 4 sessions per instance, 30-minute idle timeout, and both open and close are written to the operation audit
- **Host Bandwidth** — combined up/down rate across physical NICs, shown in the toolbar and metrics panel, exposed in `/metrics`
- **Alerts** — notify on unrecoverable exit / unhealthy verdict / successful auto-restart / memory-limit restart via DingTalk, WeCom, Feishu custom bots or your own webhook
- **Direct Execution** — Commands launch without a shell by default, so args with spaces or special characters aren't split or injected; set `shell = true` for shell features
- **File Browser** — Server-side file browsing for quick directory and executable selection
- **API Key Auth** — Optional API key protection with WebUI login dialog
- **CORS Config** — Configurable allowed cross-origin origins
- **Process Dependencies** — `depends_on` for startup ordering with topological sort
- **Hot Config Reload** — SIGHUP (Linux) or `POST /api/v1/reload` to reload config at runtime
- **Smart Updates** — Non-runtime config changes (log paths, restart policies, etc.) don't restart the process
- **Portable Layout** — Config lives next to the binary by default; logs/PID resolve against the config folder, so the whole directory can be moved as-is
- **Auto-start on Boot** — three modes: system level (Windows Task Scheduler / Windows Service / Linux systemd) and user level (no admin rights needed), with `--dry-run` preview
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

**Option 3: prebuilt portable bundle**

Download the archive for your platform from Releases (`gugu-windows-x86_64.zip` / `gugu-linux-x86_64.tar.gz`)
and unpack it. The archive holds a single executable plus the license and an example config —
**config, PID file and logs are all created next to the binary on first run**, so the folder can be moved anywhere.

After compilation, you only need a single `gugu` binary — the WebUI is embedded at compile time, no extra files required.

### Config File (Portable)

The config **follows the program**. On startup it searches in this order, and if nothing is found it generates a commented template in the program's directory:

1. Path explicitly given with `-c` (highest priority)
2. `gugu.toml` in the **same directory as the binary** (recommended)
3. `gugu.toml` in the current working directory (handy when running from the source tree)

Relative paths inside the config (logs, PID, working dir) are resolved against **the config file's directory**, so the whole folder travels with the binary without editing anything:

```
gugu-guard/
├── gugu            # single file, WebUI embedded
├── gugu.toml       # auto-generated next to the binary on first run
├── gugu.pid
└── logs/           # process logs land here too
```

You can also copy the example and edit it:

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
# Run in foreground (reads gugu.toml next to the binary; auto-created on first run)
gugu run

# Specify config file
gugu run -c /path/to/config.toml

# Set API Key via CLI (overrides config file)
gugu --api-key "your-secret-key" run
```

Open `http://localhost:9090` in your browser to access the web dashboard.

### Auto-start on Boot

Three modes — pick by "needs admin?" and "must it run before logon?":

| Mode | Platforms | Rights | Behaviour |
|---|---|---|---|
| `--mode task` (default) | Windows / Linux | admin / sudo | Windows Task Scheduler (`onstart`, as SYSTEM) / Linux systemd system unit |
| `--mode service` | Windows only | admin | Windows Service (AutoStart + crash recovery at 5s/10s/30s) |
| `--mode user` | Windows / Linux | **none needed** | Per-user at logon: Windows Run key / Linux `systemd --user` (add `loginctl enable-linger` to run before logon) |

```bash
# System level (admin / sudo)
gugu install                       # Windows: Task Scheduler
sudo gugu install                  # Linux: systemd system unit
gugu install --mode service        # Windows: register as a Windows Service

# User level (no privileges required, starts at logon)
gugu install --mode user

# Preview what would be written/executed; changes nothing
gugu install --mode task --dry-run
gugu uninstall --dry-run
```

Install performs a **permission pre-check** (it tells you to use sudo/admin, or to switch to `--mode user`),
then **reads the state back** (`systemctl is-enabled` / `schtasks /query` / `reg query` / `sc query`);
uninstall tolerates "not installed at all"; `--dry-run` only prints the plan.

> Paths containing spaces work: both the systemd unit and the Windows auto-start command quote their paths.

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
gugu run                          # Start the daemon (foreground)
gugu daemon start                 # Start detached in background (logs to file)
gugu daemon stop                  # Graceful stop (children → server → pid file)
gugu status                       # Show process status
gugu list                         # List configured processes
gugu start <name>                 # Start a process
gugu stop <name>                  # Stop a process
gugu restart <name>               # Restart a process
gugu logs <name> [-l 100]         # View logs
gugu logs <name> --level warn -q timeout --stream stderr
                                  # Live logs filtered by minimum level / keyword / stream
gugu logs <name> --file -q timeout   # Search the on-disk log file (last 4MB by default)
gugu history <name> [-m 60]       # Metric time series (CPU/memory/connections, one bucket per minute)
gugu exits <name> [-m 60]         # Exit history and availability (persisted journal)
gugu net [-m 60]                  # Host bandwidth time series
gugu audit [-l 100] [--failed]    # Operation audit (newest first; --action by code, --since in minutes)
gugu add <name> -x "node app.js"  # Add a process
gugu remove <name>                # Remove a process
gugu reload                       # Reload config file
gugu check                        # Validate config (read-only, starts nothing)
gugu status --json                # JSON output for scripts (also: list)
gugu install                      # Register auto-start (--mode task|service|user)
gugu install --mode user          # Per-user auto-start (no admin, starts at logon)
gugu install --dry-run            # Preview only, changes nothing
gugu uninstall                    # Remove auto-start (also takes --mode / --dry-run)
```

Global options:

```bash
-c, --config <PATH>      # Config file path (default: gugu.toml next to the binary)
    --server <URL>       # Server URL to connect to
    --api-key <KEY>      # API Key (overrides config file)
```

### Add Process Examples

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

`--port` accepts repeats or a comma-separated list; ports are what enable the preflight check and
connection counting. `--no-port-check` disables the preflight, `--ready-timeout` sets how long a
dependency may take to become ready (default 60s). `--cpu-max` / `--memory-max` / `--pids-max` are
cgroup v2 hard limits (Linux only, see [cgroup v2 Hard Limits](#cgroup-v2-hard-limits)).

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
| GET | `/api/v1/processes/:name/logs?lines=100&level=warn&q=keyword&stream=stderr` | Live logs, filterable by level / keyword / stream |
| GET | `/api/v1/processes/:name/logs/search?q=keyword&level=warn&limit=200&budget_mb=4` | Search on-disk logs; returns hits plus scanned lines/bytes and truncation flag |
| GET | `/api/v1/processes/:name/config` | Get config |
| GET | `/api/v1/processes/:name/history?minutes=60` | Metric time series (one bucket per minute, up to 24h) |
| GET | `/api/v1/processes/:name/exits?minutes=60&limit=50` | Exit events and availability stats |
| GET | `/api/v1/system?minutes=60` | Host bandwidth time series and current rate |
| GET | `/api/v1/audit?limit=100&action=process.start&ok=false&q=keyword&since_minutes=60` | Operation audit records (write operations, newest first) |
| GET | `/api/v1/terminal` | Terminal availability, shell, working directory, session limit and idle timeout |
| WS | `/api/v1/terminal/ws` | Built-in terminal session (JSON frames: `input` / `resize` / `data` / `exit`) |
| GET | `/api/v1/stats` | Statistics |
| GET | `/api/v1/fs/browse?path=.` | Browse filesystem |
| POST | `/api/v1/reload` | Reload config |
| POST | `/api/v1/shutdown` | Graceful daemon shutdown (stops all children first) |
| WS | `/api/v1/ws` | Real-time status |

> When `api_key` is set, all API requests require the `Authorization: Bearer <key>` header. WebSocket connections require `?token=<key>` query parameter.

## Project Structure

```
gugu-guard/
├── crates/
│   ├── core/       # Core lib: config, process management, logging, health checks, metrics & exit journal, socket probes
│   ├── server/     # Web server: REST API, WebSocket, auth, static files
│   └── cli/        # Command line tool
├── web/            # WebUI (no build step, embedded into the binary)
│   ├── index.html
│   ├── style.css
│   └── js/         # core(state/utils) / render(list) / history(metrics) / logs(viewer) / form(modals) / app(wiring+boot)
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
| `daemon.webhook_url` | string | — | Alert webhook (DingTalk / WeCom / Feishu bot or your own endpoint) |
| `daemon.webhook_format` | string | `dingtalk` | Payload shape: `dingtalk` (WeCom shares it) / `feishu` / `raw` |
| `daemon.cgroup_root` | string | `/sys/fs/cgroup` | Where cgroup v2 limits are created; point it at a writable subtree in containers or systemd delegation |
| `daemon.web.addr` | string | `0.0.0.0` | Listen address |
| `daemon.web.port` | u16 | `9090` | Listen port |
| `daemon.web.cors_origins` | string[] | `[]` | Allowed CORS origins (empty = allow all) |
| `daemon.web.terminal` | bool | `false` | Enable the built-in WebUI terminal; a non-loopback bind also requires `api_key` |

> The daemon writes its own runtime log to `gugu-daemon.<date>.log` in `log_dir`, rotated daily (last 30 kept). All three run modes (foreground, boot task, Windows service) write the file; it also prints to the console only when stdout is a terminal.

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
| `restart_reset_secs` | u64 | `60` | After the process stays up this many seconds, its crash-restart counter resets to zero, keeping the watchdog effective long-term |
| `shell` | bool | `false` | When true, command+args are concatenated and run through a shell (pipes, etc.); default launches the executable directly |
| `stop_timeout_secs` | u64 | `10` | Stop timeout in seconds |
| `depends_on` | string[] | `[]` | Process dependencies, start in dependency order |
| `max_memory_mb` | u64 | — | Process tree memory cap; exceeding it restarts via the auto-restart path |
| `cpu_max` | string | — | cgroup v2 CPU cap: `50%` / `0.5` (cores) / `50000 100000` (quota period); Linux only |
| `memory_max` | string | — | cgroup v2 memory cap: `512M` / `2G` / bytes; overrun OOMs the group; Linux only |
| `pids_max` | string | — | cgroup v2 task cap, fork-bomb protection; Linux only |
| `max_log_size_mb` | u64 | — | Log file size limit (MB), auto-rotate when exceeded |
| `unhealthy_restart` | bool | `false` | Auto-restart on health check failure |
| `listen_ports` | u16[] | `[]` | Ports this process listens on: used for the startup preflight, connection counting and dependency readiness |
| `port_check` | bool | `true` | Check that `listen_ports` are free before starting |
| `ready_timeout_secs` | u64 | `60` | How long downstream waits for this process to become ready |
| `log_alert` | table | — | Keyword/burst log alerts, see below |
| `stdout_log` | string | — | stdout log file path |
| `stderr_log` | string | — | stderr log file path |
| `log_format` | string | `auto` | Structured format for log lines: `auto` / `json` / `logfmt` / `text` |

### Health Check

```toml
[processes.my-app.health_check]
type = "tcp"              # or "http"
port = 8080               # TCP mode
# url = "http://localhost:8080/health"  # HTTP mode
interval_secs = 30
timeout_secs = 5
fail_threshold = 3        # consecutive failures before a process is judged unhealthy (only restarts with unhealthy_restart = true)
```

### Ports, Readiness and Log Alerts

```toml
[processes.my-app]
listen_ports = [8080]     # drives the preflight, connection counting and readiness of dependents
port_check = true         # fail the start when the port is already taken, naming the owner
ready_timeout_secs = 60   # cap for waiting on this process as a dependency

[processes.my-app.log_alert]
patterns = ["panic", "exception", "error"]  # any keyword hit raises an alert (lowercase substring match)
window_secs = 10          # burst window
burst_lines = 200         # alert when the window holds this many lines, 0 disables burst detection
cooldown_secs = 300       # silence for the same alert, so a retry storm can't flood the bot
```

A downstream of `depends_on` no longer waits for "the process started" but for it to actually serve:
with a health check it waits for that to pass, otherwise for every `listen_ports` entry to be listening.
On timeout, or if the dependency exits meanwhile, that downstream is skipped for this round and a
notification is sent.

> Note: connection counts are established TCP connections on those **listening ports**, read from the
> system socket table. Host bandwidth is the sum of physical NICs, not a per-process figure — the kernel
> does not account NIC bytes per process.

### Log Format & Search

```toml
[processes.my-app]
stdout_log = "logs/my-app-stdout.log"
stderr_log = "logs/my-app-stderr.log"
log_format = "auto"       # auto: try JSON → logfmt → text per line; pin it with json / logfmt / text
```

- **Parsing** — each line yields a level (`trace` / `debug` / `info` / `warn` / `error` / `fatal`;
  numeric levels accept both syslog 0-7 and bunyan/pino 10-60), a message, structured fields and an
  in-line timestamp. `level` / `msg` / `time` keys are lifted out of JSON and logfmt lines, the rest
  stays as fields, and nested objects flatten to `a.b.c`
- **ANSI handling** — the in-memory buffer, WebSocket pushes and search results have colour/cursor
  escapes stripped; files on disk keep the original bytes, so exported logs match process output
- **Live search** — `level` / `q` / `stream` on `GET /logs` filter the in-memory ring buffer
  (1000 lines per stream per process)
- **History search** — `GET /logs/search` reads the last `budget_mb` (default 4, max 32) of each log
  file, parses it with the same parser and filters it; the response carries hit counts, scanned
  lines/bytes and a truncation flag. Only the current path is read, not the `.1`…`.5` rotations
- **No invented timestamps** — a history line without its own timestamp is never given the capture
  time; a time filter simply excludes such lines

### Operation Audit

```bash
gugu audit                        # last 100 control-plane operations, newest first
gugu audit --failed --since 60    # failed operations in the last hour
gugu audit --action process.start --json
```

- **What is recorded** — every write request that changes state: create/update/delete, start/stop/restart,
  health check, log clear, group start/stop/restart, config reload and daemon shutdown. Read-only GET
  requests are not audited, so the audit panel's own query cannot loop into itself
- **One record** — time, action code and human-readable name, target, HTTP method and path, status,
  duration, source address, client (`gugu-cli/<version>` / WebUI / other User-Agent), auth outcome
  (`none` / `key` / `denied`), plus the failure reason (truncated to 300 characters)
- **Persistence** — appended to `<log_dir>/audit.jsonl` (default `logs/audit.jsonl`) and kept for 90 days:
  the retention window is applied on the first write of each day, and again immediately whenever the file
  passes 8 MB (newest half kept if still oversized). The newest 500 entries stay in memory for fast queries
- **Denied auth is recorded too** — a 401 lands as `denied` with source, target and path, so unauthorised
  attempts leave a trail
- **Source resolution** — the first hop of `X-Forwarded-For`, then `X-Real-IP`, else the peer address.
  Behind a reverse proxy, have it set those headers or you will see the proxy address. Those headers are
  client-controlled when the API is exposed directly, so only trust them behind a trusted proxy
- **Query** — `GET /api/v1/audit?limit=100&action=process.start&ok=false&q=ghost&since_minutes=60`:
  `action` is the action code, `ok=true/false` keeps successes or failures only, `q` matches target,
  path, source and reason, and `since_minutes` bounds the time window

### Built-in WebUI Terminal

```toml
[daemon]
api_key = "your-secret-key"      # required for 0.0.0.0 / LAN binds, or startup fails

[daemon.web]
terminal = true                  # default false; enables a real shell in the browser
addr = "127.0.0.1"               # loopback-only use may omit the API Key
```

- **A real PTY, not a line executor** — ConPTY on Windows and `openpty` on Unix (`portable-pty`), so the shell
  believes it is attached to a terminal: `cd`, `less`, colors, completion and TTY-only programs work. Output is
  streamed to the browser in chunks and rendered by xterm.js
- **Security gate** — the terminal is off by default. A bind on `127.0.0.1` / `localhost` / `::1` relies on local
  isolation; any other address (including `0.0.0.0`) requires `daemon.api_key` (or `GUGU_API_KEY` / `--api-key`),
  otherwise the daemon refuses to start. The check runs before the manager starts, so you never get a half-working
  daemon whose panel opens but whose terminal cannot
- **Auth and same-origin** — terminal routes live under the protected router and inherit API Key auth; the browser
  passes it as `?token=`. WebSockets are not constrained by the browser same-origin policy, so the handshake also
  requires `Origin` to match `Host`; without an API Key, `Host` must be a loopback address, which blocks DNS rebinding
  that points a malicious domain at 127.0.0.1. When disabled, `GET /api/v1/terminal` returns `{"enabled": false}` and
  the WebSocket returns 403 before upgrading
- **Session limits** — at most 4 concurrent sessions (the 5th gets 429); 30 minutes without input or output closes
  the session and reclaims the PTY; closing the panel, losing the network or pressing Close kills the child and
  releases the slot
- **Audit** — opening records `terminal.open` (status 101); closing records `terminal.close` (status 200, detail
  carries duration and shell). The User-Agent is the browser's; the source prefers `X-Forwarded-For` / `X-Real-IP` and
  falls back to `-` because a WebSocket upgrade has no peer address. The upgrade request is a GET, so the audit
  middleware does not see it; the terminal handler records it itself
- **Working directory** — new sessions start in the config file's directory, so relative paths from the config can
  be inspected directly with `cd logs`
- **Assets** — xterm.js 5.3.0 and fit addon 0.8.0 are vendored under `web/vendor/` (MIT, licenses alongside),
  embedded in the binary and never loaded from a CDN; upgrading them requires a rebuild
- **Protocol** — client `{"type":"input","data":"..."}` and `{"type":"resize","cols":120,"rows":30}`; server
  `{"type":"ready","shell","cwd","max_sessions","idle_timeout_secs"}`, `{"type":"data","data":"..."}` and
  `{"type":"exit","code","reason"}`

### cgroup v2 Hard Limits

```toml
[daemon]
cgroup_root = "/sys/fs/cgroup"   # default; point it at a writable subtree in containers or systemd delegation

[processes.my-app]
cpu_max = "50%"                  # half a core; also 0.5 (cores) or "50000 100000"
memory_max = "512M"              # overrun OOMs the whole group — this is not a restart
pids_max = "128"                 # task cap, fork-bomb protection
```

- **Prerequisites** — Linux with the cgroup v2 unified hierarchy (`cgroup.controllers` under the mount
  point) and write access: run as root, or point `cgroup_root` at a subtree delegated to you. A missing
  cpu / memory / pids controller fails the start and names the missing one
- **When it applies** — before exec, the daemon creates `<cgroup_root>/gugu/<name>-<hash>`, writes the
  limits, and the child writes itself into `cgroup.procs` between `fork` and `exec`: the whole tree is
  limited from its first instruction, with no "start it, then move it" window
- **Inheritance and cleanup** — children and grandchildren stay in the same cgroup; when the process exits
  or is stopped the daemon writes `cgroup.kill` and removes the directory, so no orphan escapes.
  A leftover directory from a previous run is cleared on the next start
- **Memory semantics** — `memory.max` is a kernel limit: crossing it triggers the cgroup OOM and the
  whole group is killed (`memory.oom.group = 1`), with `memory.swap.max = 0` so swapping cannot dodge
  the cap. The daemon only sees the exit (usually 137); `auto_restart` can bring it back
- **Versus `max_memory_mb`** — `max_memory_mb` is the daemon's own watchdog: sample, then restart.
  `memory_max` is enforced by the kernel, so the process cannot overuse before being killed. Both may be set
- **Non-Linux** — configuring any limit fails the start with an explicit "Linux only" message; it is never
  silently ignored

## CI & Release

- **CI** (`.github/workflows/ci.yml`): on Windows and Linux it runs `cargo fmt --check`,
  `cargo clippy -- -D warnings` and `cargo test`; on `master` it also mirrors branches and tags to Gitee
  (set the `GITEE_TOKEN` secret; the step skips itself when the secret is absent)
- **Release** (`.github/workflows/release.yml`): pushing a `v*` tag builds both platform bundles and
  creates a Release with `gugu-windows-x86_64.zip` / `gugu-linux-x86_64.tar.gz`
- Equivalent local gates (same as CI):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release notes.

## License

MIT — see [LICENSE](./LICENSE)
