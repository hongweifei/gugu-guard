# 更新日志

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)，日期为发布日。

## [Unreleased]

## [0.2.0] - 2026-11-28

自 0.1.0 起累计多批：服务可观测性（指标时序、退出日志、端口预检、连接数、带宽）、依赖就绪探针与日志告警、日志结构化与编码、资源硬限、未知配置键告警、操作审计、文件管理（浏览、打包下载、分片上传、解压、剪切粘贴与跨节点搬运）、WebUI 内置终端与整体重构、多节点、版本更新。0.x 阶段不追求向后兼容：配置新增字段、API 新增接口、结构体新增字段。

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

### 日志结构化与检索

- **结构化解析** —— 每行按 `log_format`（默认 `auto`，逐行回退 JSON → logfmt → 文本启发式）解析，产出级别、正文、结构化字段与行内时间；级别支持 `trace`…`fatal`，数字编码兼容 syslog 0-7 与 bunyan/pino 10-60。JSON/logfmt 的 `level`/`msg`/`time` 提升为级别、正文、时间，其余键留作字段，嵌套对象按 `a.b.c` 展平
- **ANSI 清洗** —— 内存缓冲、WebSocket 推送与检索结果里的行去掉颜色与光标控制符；落盘文件保留原始字节，导出日志与进程原样输出一致
- **服务端检索** —— `GET /api/v1/processes/:name/logs` 支持 `level` / `q` / `stream` 过滤；新增 `GET /api/v1/processes/:name/logs/search`，从最新的数据往前读，`budget_mb`（默认 4，上限 32）是整次检索的读取总量，用同一解析器过滤后返回命中条数、扫描行数/字节数与截断标记；命中都带行内时间时按时间合并两路输出，否则保留文件顺序（先 stdout 后 stderr，每条流内旧备份在前、当前文件在后）
- **轮转备份检索** —— 检索范围覆盖当前文件与 `.1`…`.6` 六份轮转备份，预算优先给最新的数据：当前文件读满再翻上一份，更旧的备份不再读并标记截断；「日志文件尚未生成」的判定同样把只剩备份的情况算作已落盘。轮转命名收进 `logs::rotate`（`rotated_path` / `MAX_BACKUPS`），写盘与检索共用 `stem.N.ext` 一套规则
- **时间口径** —— 历史行解析不出自己的时间戳就不填采集时间；按时间过滤时这类行被排除，不编造
- **CLI** —— `gugu logs` 增加 `--level` / `-q` / `--stream`，`--file` 切到历史文件检索并打印扫描统计，`--budget-mb` / `-n` 控制预算与条数；`gugu add` 增加 `--log-format`
- **WebUI** —— 日志面板增加级别下拉、每行级别徽标、结构化字段胶囊与「历史检索」按钮（附扫描统计与「回到实时」）；进程表单暴露日志格式
- **配置** —— 进程配置新增 `log_format`（`auto` / `json` / `logfmt` / `text`）；`LogEntry` 增加可选 `parsed` 字段，WS 载荷在纯文本行上保持原样
- **仓库** —— `.gitignore` 的 `logs/` 改为锚定根目录的 `/logs/`，源码目录 `crates/core/src/logs` 不再被误忽略

### 资源硬限（cgroup v2 / Job Object）

- **每进程独立 cgroup** —— Linux 下启动前创建 `<cgroup_root>/gugu/<名>-<哈希>`，把 `cpu.max` / `memory.max` / `pids.max` 写好后，子进程在 `fork` 之后、`exec` 之前写入自己的 pid：进程树从第一条指令起就在限值内，没有「先跑再搬」的空窗
- **限值写法** —— `cpu_max` 支持 `50%`、`0.5`（核数）与 `50000 100000`（quota period），换算与内核校验（周期 1000..=1000000µs、配额 ≥1000µs）在解析层完成；`memory_max` 支持 `512M` / `2G` / 字节数（1024 进制，最低一页）；`pids_max` 为正整数。空白或 `max` 表示不限
- **内存语义** —— Linux 的 `memory.max` 由内核执行，越过即整组 OOM（`memory.oom.group = 1`），并写 `memory.swap.max = 0` 防止靠换页绕过上限；守护进程只观测到进程退出，`auto_restart` 照常生效
- **继承与清理** —— 子进程与孙进程留在同一 cgroup；进程退出、停止或启动失败时对整个 cgroup 执行 `cgroup.kill`、等其清空后删目录；上次运行留下的同名目录在下次启动前先清空
- **Windows 映射** —— 同一组字段落到 Job Object：`memory_max` 是 Job 提交内存上限（`JobMemoryLimit`），`pids_max` 是活动进程数上限（`ActiveProcessLimit`），`cpu_max` 是 CPU 速率硬顶（`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`）。限值在创建 Job 时写入，主进程 `spawn` 后立即分配进 Job，之后派生的子进程自动继承
- **Windows 口径** —— CPU 按整机占比节流（`cpu_max = "10%"` 在 16 逻辑核机器上是整机 0.625%，启动日志打印实际占比），是限速而不是杀进程；内存越界时新的分配失败，进程通常自行报错退出；进程数只统计进程，不统计线程
- **接口** —— 进程配置新增 `cpu_max` / `memory_max` / `pids_max`，守护进程配置新增 `cgroup_root`；POST/PUT 进程接口接受这三个字段，`gugu add` 增加 `--cpu-max` / `--memory-max` / `--pids-max`，WebUI 表单新增「资源硬限」区块。既不是 Linux 也不是 Windows 的平台配置硬限会在启动时明确失败，不静默忽略

### 未知配置键告警

- **键集合比对** —— 加载时把原始 TOML 与「按结构体序列化出的已知键集合」逐层比对（用 JSON 承载，`None` 字段会保留成 `null`，字段名清单才完整），得到只在文件里出现的键的完整路径，如 `processes.web.memory_maxx`
- **拼写建议** —— 在同一层内按编辑距离找最接近的字段，距离足够近才给出建议：`memory_maxx（是否想写 memory_max）`，过远的匹配不猜
- **告警而不阻断** —— 启动日志逐条 `WARN`；`gugu check` 以 `!` 列出并在结论里标注「N 个未知键被忽略」，退出码仍为 0；SIGHUP、`gugu reload` 与 `POST /api/v1/reload` 重载时同样告警

### 文件管理

- **白名单是一切的前提** —— `daemon.web.fs_roots` 限定范围：请求路径先解析真实路径（`..` 与符号链接都已展开），再按路径分量与每个根比较，`/data` 不会放行 `/database`；相对条目以配置文件所在目录为基准。越界返回 403 并带上被拒绝的路径
- **读写分权** —— 新增 `daemon.web.fs_write`（默认 `false`）：只读时浏览、预览、下载照常可用，写入口统一 403 并在消息里指出开启位置；开启后与内置终端同一道门槛——监听非回环地址必须配 `api_key`，否则拒绝启动
- **浏览与预览** —— `GET /fs/list` 列出目录（目录优先、名称不区分大小写）、`GET /fs/stat` 取单个路径元信息、`GET /fs/read` 预览文本（1 MB 上限，UTF-8 失败按 GBK 解，前 8 KB 出现 0 字节判为二进制）、`GET /fs/download` 流式下载并带 RFC 5987 的 UTF-8 文件名
- **目录打包下载** —— `GET /fs/archive` 把一个目录递归打成 zip：归档在系统临时目录生成，响应结束即删除，符号链接按链接自身跳过，原始内容超过 2 GB 时提示分目录下载。目录默认套一层同名目录，`flat=true` 时内容直接落在归档根；单个文件也能打包，归档里就是它自己一个条目
- **解压归档** —— `POST /fs/extract` 把 zip、tar 或 tar.gz / tgz 解到归档旁的同名目录（可显式指定目标），目标已存在需 `overwrite`，覆盖是整目录替换。条目先解到 `.gugu-unpack-` 临时目录，全部成功后再整体改名就位；归档里只有一个与目标同名的顶层目录时直接展开它，不会解成 `foo/foo`，`collapse=false` 可关掉这条展开、保持归档原样；条目名里的 `..`、绝对路径与盘符跳过，符号链接与设备文件条目也跳过，解压后总量上限 2 GB、条目数上限 2 万
- **剪切与粘贴** —— WebUI 行按钮「剪切」把来源与来源节点记进标签页的会话存储，工具条出现「粘贴 <名字>」与「取消」，被剪切的条目在列表里留一条强调色。粘贴目标是当前目录 + 原名，同一节点走 `/fs/rename`，跨节点走 `/fs/transfer`；目标已存在先确认再带 `overwrite` 重试。剪贴板只有一项，切换节点时保留，跨节点搬完再粘是常见用法
- **跨节点搬运** —— `POST /fs/transfer` 由中心节点编排四步：源侧打包（目录平铺、单文件直接取内容）→ 中心节点系统临时目录中转 → 目标侧落地（目录解压、单文件写入后改名）→ 删源。成功后才删源，删源失败返回 500 并说明内容已到达目标；源不存在、源是符号链接、目标父目录不存在、目标已存在而未声明覆盖都在开始前拦下。两端同节点返回 400 并指向 `/fs/rename`，避免同一件事两条实现。中转归档用 `.gugu-transfer-*` / `.gugu-move-*` / `.gugu-part-*` 命名，列表与打包照旧跳过
- **写操作** —— `POST /fs/upload`（请求体即文件内容，边收边写同目录临时文件再改名就位，目标已存在需 `overwrite`）、`PUT /fs/write`（保存文本，8 MB 上限）、`POST /fs/mkdir` / `fs/touch` / `fs/rename` / `fs/copy`、`DELETE /fs`（目录需 `recursive=true`）。白名单根自身不允许删除或改名
- **分片上传与续传** —— `POST /fs/upload/session` 开会话、`PUT /fs/upload/session/{id}?offset=N` 顺序追加、`GET` 查已收字节数、`POST .../finish` 校验总量后改名、`DELETE` 放弃并删除临时文件。偏移由服务端记录并校验，重发上一片返回 400，半片失败会截回上一次成功的偏移；空闲超过 24 小时的会话在下次开会话时回收
- **进度与取消** —— WebUI 上传用 XHR 取实时进度，下载按 `Content-Length` 与已读字节算百分比；传输条显示文件名、百分比与大小。取消只中止请求，已传字节留在服务端会话里，重新选中同一个文件从断点接着传，「放弃」才删掉会话。WebUI 对超过 8 MB 的文件自动改走分片，单片失败从服务端偏移重试
- **拖拽与整目录上传** —— 文件和文件夹拖进文件视图即上传，目录按原结构先建后传；「上传目录」按钮走同一套逻辑
- **中转文件不外露** —— 上传与分片临时文件统一用 `.gugu-` 前缀，目录列表与打包都跳过它们，面板里不会出现半成品
- **缺省与告警** —— 未给起始目录时取第一个根，避免默认工作目录不在白名单的情况；非回环监听且白名单为空时，启动日志提示这条能力的暴露面
- **审计** —— 动作码细化到 `fs.upload` / `fs.upload.begin` / `fs.upload.chunk` / `fs.upload.abort` / `fs.write` / `fs.extract` / `fs.transfer` / `fs.mkdir` / `fs.create` / `fs.rename` / `fs.copy` / `fs.delete`，对象记录被操作的具体路径
- **CLI 与 WebUI** —— `gugu fs ls / cat / get / put / extract / mkdir / touch / mv / cp / rm`，`get -r` 与 `put -r` 递归处理目录并保留结构，终端里的传输显示百分比；`fs extract --no-collapse` 保留归档顶层目录，`fs mv --from-node <节点>` 把条目从另一台节点搬过来（缺省与 `--node` 同节点时仍是改名）；侧栏「文件」视图左侧目录、右侧预览编辑器，支持拖拽上传、整目录上传、打包下载、归档解压、剪切粘贴与传输取消，只读时关闭写入口
- **体检与模板** —— `gugu check` 打印每个根目录是否存在，空条目算配置错误；默认模板与 `config.example.toml` 给出根目录与 `fs_write` 的注释示例
- **不写回文件** —— `AppConfig.unknown_keys` 标了 `#[serde(skip)]`，`gugu add` 等写回操作不会把它带进配置
- **旧接口下线** —— `/api/v1/fs/browse` 与 `BrowseResponse` / `FsEntry` 一并移除，进程表单的「选择文件 / 选择目录」改用 `/fs/list`

### 操作审计

- **审计中间件** —— 只拦 POST / PUT / DELETE，按方法 + 路径映射成稳定动作码与中文名（`process.start` / `group.restart` / `config.reload` / `daemon.shutdown` 等），目标名从路径取出；未知写接口记为 `http.write`，新增端点不会静默漏记
- **一条记录** —— 时间、动作码与中文名、对象、方法与路径、状态码、耗时、来源地址、客户端 User-Agent、认证结果（`none` / `key` / `denied`）与失败原因。401 也留痕，来源与目标都在
- **落盘与保留** —— 追加到 `<log_dir>/audit.jsonl`，保留 90 天：每天首次写入时按保留期清理一次，文件超过 8MB 再立即压缩（仍过大就只留较新的半份）。内存另留最近 500 条供接口直接查
- **来源口径** —— 优先 `X-Forwarded-For` 第一跳，其次 `X-Real-IP`，否则用连接对端地址；CLI 请求带 `gugu-cli/<版本>` UA，审计里能区分 CLI 与 WebUI
- **接口与工具** —— `GET /api/v1/audit` 支持 `limit` / `action` / `ok` / `q` / `since_minutes`；`gugu audit` 支持 `--limit` / `--action` / `--failed` / `--since` / `--json`；WebUI 顶栏新增「操作审计」面板，可按动作、时间范围、结果与关键字筛选

### WebUI 内置终端

- **真 PTY** —— `portable-pty` 在 Windows 上走 ConPTY、Unix 上走 `openpty`，每个 WebSocket 连接一个独立 shell；`cd`、分页器、彩色输出与交互式程序都按终端语义工作，输出按块实时回传
- **开关与安全门槛** —— 配置新增 `daemon.web.terminal`（默认 `false`）；开启后若监听地址不是回环且没有 `api_key`，守护进程在 manager 启动前直接拒绝启动并说明两种修法。终端路由挂在受保护路由下，自动继承 API Key 认证，WS 用 `?token=` 传 Key；握手强制 `Origin` 与 `Host` 同源，没有 Key 时 Host 必须是回环，挡住跨站 WebSocket 与 DNS rebinding
- **会话治理** —— 单实例最多 4 个并发会话，超出返回 429；30 分钟无输入无输出自动结束；断线、关面板或主动关闭都会 kill 子进程并归还会话名额
- **审计** —— 打开写 `terminal.open`（101），关闭写 `terminal.close`（200，detail 记时长与 shell）；GET 升级请求由 handler 自己记录，不依赖只拦写操作的审计中间件
- **审计落盘并发修复** —— 终端会话天然并发，审计追加改为单锁内一次写入完整 JSON 行，避免多个会话同时记录时两行粘在一起
- **接口与前端** —— 新增 `GET /api/v1/terminal` 与 `WS /api/v1/terminal/ws`；WebUI 顶栏「终端」按钮按开关状态显示，全屏 xterm.js 面板贴合暗色主题，窗口尺寸变化时 fit addon 重算并同步 `resize`。xterm.js 5.3.0 / fit addon 0.8.0 vendor 到 `web/vendor/`，随二进制嵌入，不访问 CDN

### 多节点

- **配置** —— 守护进程配置新增 `daemon.node_name`（本机显示名，缺省用主机名），顶层新增 `[[nodes]]`：`name`（节点标识，只能用字母/数字/`.`/`_`/`-`，保留 `local`，且不能是 `.` 或 `..`）、`label`、`url`、`api_key`。指向非回环地址却没配 Key 时，`gugu check` 与守护启动都直接报错——代理会用这把 Key 操作从节点，不能默认裸奔
- **节点表与探测** —— 本机固定为 `local`，其余按配置顺序排列；后台每 5 秒探测各从节点的 `GET /api/v1/node`（3 秒超时），记录在线/离线、往返延迟、版本、主机名、平台与进程数，`GET /api/v1/nodes` 一次返回全部
- **HTTP 代理** —— `ANY /api/v1/nodes/{node}/{*rest}` 转发到 `{url}/api/v1/{rest}`：方法、查询串、请求体、`Content-Type` 与下载响应头原样传递，认证换成从节点自己的 Key；响应边收边转，只对建连与空闲读取设超时；未知节点 404、本机节点 400、从节点不可达 502 并带上原因
- **实时推送中继** —— `WS /api/v1/ws?node=` 由中心节点连上从节点的 `/api/v1/ws` 后双向中继，浏览器收到的仍是 `status` / `logs` 两类消息，切换节点即切换数据源
- **审计** —— 代理路径的写操作在中心节点留痕：动作码与本机一致，对象前缀节点名（`web1/api`），重载、停止守护这类无对象动作记节点名本身；从节点自身的审计里客户端类型照原样保留
- **WebUI** —— 顶栏节点选择器带在线状态圆点，切换后进程列表、日志、指标、审计与表单全部指向该节点并重连实时推送，已打开的浮层先关闭；离线节点在页面顶部显示最后一条错误。内置终端只操作本机，选中从节点时入口隐藏
- **CLI** —— 新增 `gugu nodes`（节点、地址、在线状态、延迟、版本、进程数，支持 `--json`）；全局 `--node <name>` 让所有客户端命令走中心节点代理，节点标识按 URL 路径段转义，`.`、`..` 在发请求前报错
- **写回与重载** —— 增删进程写回配置时保留 `[[nodes]]` 原样；`gugu reload` 会重载节点配置，但节点表在守护启动时建立，改动后需重启守护进程生效
- **来源链路** —— 代理与 WebSocket 中继都带 `X-Forwarded-For`：请求自带的链路原样保留、中心节点把直连对端追加在末尾，从节点的审计与终端会话记的是原始客户端而不是中心节点

### 日志编码

- **问题** —— 中文 Windows 的程序把 GBK 写进 stdout（`ping`、cmd 内部命令、Python 默认编码），采集侧此前按 `String::from_utf8_lossy` 解码，中文整行变成 `�`
- **解码层** —— 新增 `LogEncoding`：`auto`（先按 UTF-8 严格解码，失败再按 GBK）/ `utf-8` / `gbk`，配置里也接受 `utf8`、`cp936`；非法字节替换成 `U+FFFD`，整行不丢
- **采集与检索** —— 采集按字节切行、逐行解码后才进内存缓冲与广播；历史检索同样按字节切行再逐行解码，GBK 双字节不会被整块 lossy 糊掉
- **落盘不变** —— 日志文件始终写程序输出的原始字节，导出下载对得上原样；解码只作用于面板、WebSocket 与检索
- **接口与工具** —— 进程配置新增 `encoding`（默认 `auto`），`POST/PUT /api/v1/processes/:name` 接受该字段，`gugu add` 增加 `--encoding`，WebUI 表单新增「日志编码」下拉

### 结构去重（内部）

- **字节格式化** —— 新增 `core::units::format_bytes` 作为唯一实现（1024 进制，非 B 单位保留一位小数，覆盖到 TB）；cgroup 限值描述、进程资源用量与 CLI 的体积列都调用它，三处各自的格式化代码随之删除，数值口径保持一致
- **URL 权威段** —— 新增 `core::net::split_authority`，从任意写法里取出主机与端口（去 scheme、userinfo 与路径，IPv6 支持 `[::1]:9090` 与裸 `::1`）；`authority_host` 与 `config::port_from_url` 都由它派生，端口缺失时按 scheme 回落 80/443
- **从节点表** —— `UpstreamNode` 持有一份 `NodeConfig` 并在构造时统一裁剪空白、丢弃空标签与空 Key，字段通过方法读取；节点表、探测、代理与视图四处共用这一份配置模型，不再各自搬运四个平行字段
- **服务状态** —— 删除 `ServerOptions`，认证、跨域、审计目录、终端、白名单与节点表都收进 `AppState`，由链式构造一次成型；节点配置在 `with_nodes` 里完成校验，`run_server` 只接收 `AppState` 与停止信号

### 配置与进程状态机拆分（内部）

- **配置模块** —— 原 `config.rs` 914 行按职责拆成 `config/{mod,daemon,node,process,paths,unknown,write}`：`AppConfig` 只保留加载、保存与告警；守护进程与 Web 配置、从节点与节点表校验、进程配置与健康检查、路径查找、未知键检出、写回合并各自成文件。对外路径由 `mod.rs` 统一 re-export，调用方无需改动
- **进程状态机** —— 原 `process/managed.rs` 694 行按动作拆成 `managed/{mod,spawn,lifecycle,observe,view}`：字段与基础访问留在 `mod`，启动（命令构造、硬限落地、日志采集接线）、停止与存活检查、节拍采样、对外视图分块实现同一类型，单个文件都不再承载整条生命周期

### 安装、套接字与表单拆分（内部）

- **安装命令** —— 原 `install.rs` 616 行拆成 `install/{mod,units,windows,unix}`：`mod` 只留模式解析与安装/卸载入口、配置模板生成和跨平台小工具；`units` 放外部命令执行与 systemd 单元文本构造（连同其构造测试）；`windows` 放任务计划、Run 键与 Windows 服务；`unix` 放 systemd 系统级与用户级单元的安装卸载。平台分支各归其文件，`--mode service` 在 Unix 上明确拒绝
- **套接字** —— 原 `sockets.rs` 464 行拆成 `sockets/{mod,parse,windows,unix}`：`mod` 保留 `Socket`/`Listener` 与公共查询；`parse` 放 `/proc/net/tcp` 行解析与 `TcpState`；`windows` 与 `unix` 分别承载 `GetExtendedTcpTable`/ToolHelp 与 `/proc` 读取，两个平台同名挂为 `platform`，公共函数签名不变
- **指标渲染** —— 原 `metrics.rs` 400 行拆成 `metrics/{mod,render,tests}`：`mod` 只保留快照容器、路由与处理器；`render` 按指标族拆成函数（CPU、健康、内存、连接、重启、状态、时长、聚合、带宽），拼装顺序与输出文本保持不变；渲染测试独立成 `tests`
- **前端表单** —— 原 `web/js/form.js` 434 行拆成三个脚本：`form.js` 保留模态框、动态参数与环境变量行、命令预览、重名检查、编辑回填与提交；`form-health.js` 放健康检查与网络设置的收集/回填/重置；`form-fs.js` 放文件系统浏览器整体。`index.html` 按依赖顺序引入，共享全局作用域

### WebUI 重构

- **信息架构** —— 左侧导航（节点切换、进程 / 操作审计 / 终端、连接状态与版本）+ 主区进程表格 + 右侧进程抽屉。抽屉用三个标签承载概览、日志与指标，取代原先的日志浮层与指标浮层；审计与终端从面板提升为整页视图；表格列收敛为八列（进程、PID、运行时长、资源、端口与连接、上次退出、健康、操作）
- **交互** —— 统计条前四项可点选筛选，与搜索词、分组芯片三重过滤取交集；分组芯片内联批量启停；行内「更多」菜单改为挂在 `body` 上的 fixed 定位浮层，不再被表格容器裁剪；按钮统一 loading 态；`/` 聚焦搜索框；Esc 只关闭浮层栈顶并归还焦点
- **视觉** —— 深色控制台延续既有配色并令牌化，去掉 Google Fonts 改为系统字体栈 + `ui-monospace`，离线部署不再依赖外网；折叠搜索、stats、分组、抽屉、表单与终端的间距与断点（1180 / 1120 / 960 / 860 / 640）
- **代码组织** —— 前端改为原生 ES 模块，依赖方向单向 core → ui → views / panels / forms → main：`core`（`api` / `state` / `live` / `format`）、`ui`（`dom` / `icons` / `toast` / `overlay` / `dialog` / `drawer`）、`views`（`sidebar` / `toolbar` / `stats` / `groups` / `processes`）、`panels`（`overview` / `logs` / `metrics` / `audit` / `terminal`）、`forms`（`fields` / `dynamic` / `health` / `network` / `alerts` / `fs` / `process`）。模块之间通过 `state` 订阅与 `main` 注入的回调通信，不再有全局函数与内联 `onclick`
- **样式** —— 单一 `style.css` 拆成 `css/{tokens,layout,components,panels}.css`：令牌与重置、页面骨架、通用控件、面板与表格细节各自成文件，跨文件不重复定义选择器
- **契约对齐** —— 概览的「最近退出」只展示 `ProcessInfo.recent_exits` 实际携带的三项（时间、退出码、是否自动拉起），存活时长与主动停止只在 `/processes/{name}/exits` 的五列统计里出现；表单不再读取 `ProcessInfo.max_restarts`
- **表单** —— 底栏从滚动区移出，内容区独立滚动、按钮恒定可见；健康检查按类型显示 TCP / HTTP 字段，未启用时通用字段整体禁用并弱化；编辑提交以原配置为底稿，未在表单暴露的 `depends_on`、`max_memory_mb`、`max_log_size_mb` 原值带回
- **文件浏览** —— 目录模式只列目录，文件模式目录用于导航、文件用于选中；对话框关闭时归还焦点

### 版本更新

- **版本模型** —— 新增 `Version`：解析 `x.y.z[-pre]`，接受 `v` 前缀与 `+build` 后缀，预发布段按 `.` 分段、数字段按数值比较（`1.0.0-rc.10 > 1.0.0-rc.2`），无预发布段高于任何预发布；不引入 semver 依赖
- **发布源** —— `github` / `gitee` 读各自的 release 列表接口并跳过草稿与预发布，`manifest` 读自建 JSON（`version` / `notes` / `assets`）；`mirror` 只改写安装包地址，清单仍直连
- **平台资产** —— 由资产名 `gugu-<平台>.zip|tar.gz` 推出 key（如 `windows-x86_64`）后精确匹配，不跨架构回退
- **下载与校验** —— 流式下载后按 zip / tar.gz 解包出二进制，先跑 `--version` 要求输出与目标版本一致，资产带 `digest`（GitHub）或清单给 `sha256` 时再核对安装包摘要
- **替身替换** —— 从暂存目录 `<程序目录>/.gugu-update/<版本>/` 启动同一份新二进制的隐藏子命令 `__apply-update`：等旧 PID 退出（上限 60 秒）→ 旧文件改名 `.bak` → 新文件就位 → 按需 `gugu daemon start`，自身输出写 `apply.log`。换入失败且目标已让位时把 `.bak` 放回原位
- **端口接管** —— 递到新守护进程的句柄为零继承（Windows 走 `CreateProcessW`，`bInheritHandles = FALSE`，独立进程组与隐藏窗口），监听套接字建好后标记为不可继承，端口随旧进程退出释放；地址仍在 `TIME_WAIT` 时按 500 毫秒步长重试至多 30 秒，占用者是另一个活着的进程时直接报错
- **自动更新** —— `daemon.update.auto` 三档 `off` / `check` / `install`（默认 `off`），启动 30 秒后首次检查、之后按 `check_hours`（默认 24）轮询；`restart`（默认 `true`）与 `keep_backup`（默认 `true`）可配
- **接口** —— `GET /api/v1/update`、`POST /api/v1/update/check`、`POST /api/v1/update/apply`（`{"version":"…"}` 可空，202；已被占用或不可用时 409）、`POST /api/v1/update/settings`（写回 `auto` 一个键，toml_edit 保留注释与其余条目）；未启用更新服务时四端点返回 501，检查失败返回 502
- **CLI** —— `gugu update --check` / `gugu update [--version x.y.z]`，安装后轮询 `/api/v1/node` 至多 120 秒，等新版本上线再退出；`gugu check` 增加更新配置体检
- **WebUI** —— 侧栏版本号成为入口（有新版时点亮圆点），弹窗展示当前与最新版本、发布时间、更新说明、自动策略与安装进度，每 1.5 秒轮询进度
- **依赖** —— 新增 `zip`（仅 deflate）、`tar`、`flate2`、`sha2`

### 修复

- `gugu add` 与 `POST /api/v1/processes/:name` 接受 `log_format: null`：CLI 未指定日志格式时发的就是 null，此前会被请求体校验拒绝（返回 400）
- 改完 `web/` 再 `cargo build` 会重新嵌入前端资源：`crates/server/build.rs` 声明了对 `web/` 的依赖，rust-embed 的输入不再游离在 cargo 的依赖范围之外

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

- 默认监听 `0.0.0.0:9090` —— **暴露到公网前请设置 `daemon.api_key` 并限制网络访问**
- 0.x 期间配置文件名、CLI 参数与 API 形状仍可能调整

[0.2.0]: https://github.com/hongweifei/gugu-guard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hongweifei/gugu-guard/releases/tag/v0.1.0
