//! 日志行结构化解析：JSON / logfmt / 文本启发式，统一产出级别、正文、字段与行内时间。
//!
//! 解析是纯函数、不碰 I/O：采集侧对实时行调用一次，历史文件检索侧对落盘行调用一次，
//! 两边因此共用同一口径。任何一行解析失败都只是「没有结构化信息」，行本身照常保留。

use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// 日志级别，按严重程度排序：`Trace < Debug < Info < Warn < Error < Fatal`。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LogLevel {
    #[default]
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub const ALL: [Self; 6] = [
        Self::Trace,
        Self::Debug,
        Self::Info,
        Self::Warn,
        Self::Error,
        Self::Fatal,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }

    /// 从日志里的级别 token 解析：容忍大小写、方括号、冒号等装饰。
    ///
    /// 数字级别同时接受 syslog（0-7）与 bunyan/pino（10-60）两套编码。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let normalized: String = raw
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        match normalized.as_str() {
            "trace" | "trc" | "verbose" | "vrb" => Some(Self::Trace),
            "debug" | "dbg" => Some(Self::Debug),
            "info" | "inf" | "information" | "notice" => Some(Self::Info),
            "warn" | "warning" | "wrn" => Some(Self::Warn),
            "error" | "err" | "eror" | "exception" => Some(Self::Error),
            "fatal" | "crit" | "critical" | "panic" | "severe" | "emerg" | "emergency"
            | "alert" => Some(Self::Fatal),
            other => other.parse::<i64>().ok().and_then(level_from_number),
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// syslog severity（0-7）与 bunyan/pino（10-60）两套数字级别编码。
fn level_from_number(n: i64) -> Option<LogLevel> {
    const SYSLOG: [LogLevel; 8] = [
        LogLevel::Fatal,
        LogLevel::Fatal,
        LogLevel::Fatal,
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Info,
        LogLevel::Debug,
    ];
    if let Ok(index) = usize::try_from(n) {
        if let Some(level) = SYSLOG.get(index) {
            return Some(*level);
        }
    }
    match n {
        10 => Some(LogLevel::Trace),
        20 => Some(LogLevel::Debug),
        30 => Some(LogLevel::Info),
        40 => Some(LogLevel::Warn),
        50 => Some(LogLevel::Error),
        60 => Some(LogLevel::Fatal),
        _ => None,
    }
}

/// 日志行的结构化格式；`Auto` 按 JSON → logfmt → 文本的顺序逐档尝试。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LogFormat {
    #[default]
    Auto,
    Json,
    Logfmt,
    Text,
}

impl LogFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Json => "json",
            Self::Logfmt => "logfmt",
            Self::Text => "text",
        }
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一行日志的结构化信息。字段都是可选的：解析不出就当没有，不编造。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedLine {
    /// 级别；结构里没有级别字段、文本行里也没有级别 token 时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<LogLevel>,
    /// 去掉时间戳、级别、字段后剩下的正文；与原始行一致时省略，避免传输重复内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 结构化字段（JSON 键或 logfmt 键值对，已排除被提升为 level/message/at 的键）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    /// 行内自带的时间戳；解析不出时为 None（此时不要拿采集时间去冒充）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

impl ParsedLine {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.level.is_none()
            && self.message.is_none()
            && self.fields.is_empty()
            && self.at.is_none()
    }
}

const LEVEL_KEYS: [&str; 7] = [
    "level",
    "lvl",
    "severity",
    "log.level",
    "loglevel",
    "levelname",
    "level_name",
];

const MESSAGE_KEYS: [&str; 4] = ["msg", "message", "event", "text"];

const TIME_KEYS: [&str; 6] = ["timestamp", "time", "ts", "@timestamp", "datetime", "date"];

/// 清洗一行：去掉 ANSI 颜色/光标控制序列与不可见控制字符，把制表符换成空格。
///
/// 落盘文件保留原始字节，清洗只作用于内存、广播与检索看到的那一份。
#[must_use]
pub fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\t' => out.push(' '),
            '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}' => {}
            _ => out.push(c),
        }
    }
    out.trim_end().to_string()
}

/// 吃掉一个转义序列：CSI（`ESC [` … 终止字节）、OSC（`ESC ]` … BEL/`ESC \`）、其余两字节序列。
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        // `ESC (` / `ESC )` 之类的字符集切换带一个参数字节
        Some('(' | ')' | '#') => {
            chars.next();
        }
        _ => {}
    }
}

/// 按声明的格式解析一行；`Auto` 逐档回退。
#[must_use]
pub fn parse_line(line: &str, format: LogFormat) -> ParsedLine {
    let trimmed = line.trim_end();
    match format {
        LogFormat::Json => parse_json(trimmed)
            .or_else(|| parse_text(trimmed))
            .unwrap_or_default(),
        LogFormat::Logfmt => parse_logfmt(trimmed, true)
            .or_else(|| parse_text(trimmed))
            .unwrap_or_default(),
        LogFormat::Text => parse_text(trimmed).unwrap_or_default(),
        LogFormat::Auto => parse_json(trimmed)
            .or_else(|| parse_logfmt(trimmed, false))
            .or_else(|| parse_text(trimmed))
            .unwrap_or_default(),
    }
}

/// JSON 对象行：键值展平为字段，`level`/`msg`/`time` 提升为级别、正文、时间。
fn parse_json(line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let object = value.as_object()?;
    if object.is_empty() {
        return None;
    }
    let mut fields = BTreeMap::new();
    flatten_json(object, "", &mut fields, 0);
    Some(finish_fields(fields, ""))
}

const MAX_FIELD_DEPTH: usize = 3;

/// 嵌套对象按 `a.b.c` 展平；超过 [`MAX_FIELD_DEPTH`] 的层压成一行紧凑 JSON。
fn flatten_json(
    map: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
    depth: usize,
) {
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            serde_json::Value::Object(inner) if depth < MAX_FIELD_DEPTH => {
                flatten_json(inner, &path, out, depth + 1);
            }
            _ => {
                if let Some(text) = scalar_text(value) {
                    out.insert(path, text);
                }
            }
        }
    }
}

fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => None,
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).ok()
        }
    }
}

/// logfmt 行：`key=value` 对，值可带双引号与转义；无等号的裸词作为正文。
fn parse_logfmt(line: &str, lenient: bool) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 开头就没有等号的行不可能是 logfmt，先挡掉，省掉逐行切词的分配。
    if !trimmed.as_bytes().iter().take(96).any(|byte| *byte == b'=') {
        return None;
    }
    let (pairs, leftovers) = split_logfmt(trimmed);
    if pairs.is_empty() {
        return None;
    }
    // 自动识别时要求「至少两个键值对」或「单个键是公认结构化键」，
    // 避免把 `PATH=C:\bin` 这类普通文本误判成 logfmt。
    if !lenient && pairs.len() < 2 && !pairs.iter().any(|(key, _)| is_known_key(key)) {
        return None;
    }
    let mut fields = BTreeMap::new();
    for (key, value) in pairs {
        fields.insert(key, value);
    }
    Some(finish_fields(fields, &leftovers.join(" ")))
}

fn is_known_key(key: &str) -> bool {
    LEVEL_KEYS.contains(&key) || MESSAGE_KEYS.contains(&key) || TIME_KEYS.contains(&key)
}

fn split_logfmt(line: &str) -> (Vec<(String, String)>, Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut pairs = Vec::new();
    let mut leftovers = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let key_start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '=' {
            leftovers.push(chars[key_start..i].iter().collect());
            continue;
        }
        let key: String = chars[key_start..i].iter().collect();
        i += 1;
        let mut value = String::new();
        if i < chars.len() && chars[i] == '"' {
            i += 1;
            while i < chars.len() {
                match chars[i] {
                    '"' => {
                        i += 1;
                        break;
                    }
                    '\\' if i + 1 < chars.len() => {
                        value.push(match chars[i + 1] {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                        i += 2;
                    }
                    c => {
                        value.push(c);
                        i += 1;
                    }
                }
            }
        } else {
            let value_start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            value.extend(chars[value_start..i].iter());
        }
        if !key.is_empty() {
            pairs.push((key, value));
        }
    }
    (pairs, leftovers)
}

/// 把字段表收窄成 [`ParsedLine`]：提升级别/正文/时间，其余留在 `fields`。
fn finish_fields(mut fields: BTreeMap<String, String>, leftover: &str) -> ParsedLine {
    let at = take_key(&mut fields, &TIME_KEYS)
        .and_then(|raw| parse_timestamp(&raw).or_else(|| parse_epoch(&raw)));
    let level = take_key(&mut fields, &LEVEL_KEYS).and_then(|raw| LogLevel::parse(&raw));
    let message = take_key(&mut fields, &MESSAGE_KEYS)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            let text = leftover.trim();
            (!text.is_empty()).then(|| text.to_string())
        });
    ParsedLine {
        level,
        message,
        fields,
        at,
    }
}

fn take_key(fields: &mut BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| fields.remove(*key))
        .filter(|value| !value.trim().is_empty())
}

/// 文本行启发式：行首时间戳（可带方括号）+ 级别 token，剩下的就是正文。
fn parse_text(line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut rest = trimmed;
    let mut at: Option<DateTime<Utc>> = None;

    // `[2026-11-26 09:00:00] ...`
    if let Some(stripped) = rest.strip_prefix('[') {
        if let Some(close) = stripped.find(']') {
            if let Some(dt) = parse_timestamp(&stripped[..close]) {
                at = Some(dt);
                rest = stripped[close + 1..].trim_start();
            }
        }
    }

    if at.is_none() {
        if let Some((token, after)) = take_token(rest) {
            let bare = trim_token_punctuation(token);
            if let Some(dt) = parse_timestamp(bare) {
                at = Some(dt);
                rest = after;
            } else if NaiveTime::parse_from_str(bare, "%H:%M:%S%.f").is_ok() {
                // 只有时分秒（log4j 风格）：不猜日期，只吞掉这一截，级别仍在后面
                rest = after;
            } else if let Some((second, after2)) = take_token(after) {
                let joined = format!("{bare} {}", trim_token_punctuation(second));
                if let Some(dt) = parse_timestamp(&joined) {
                    at = Some(dt);
                    rest = after2;
                }
            }
        }
    }

    // 时间之后、级别之前常有一个上下文标记（`[main]`、`[pid 123]`）：认作正文前缀，
    // 不当作级别，也不丢掉。
    let mut prefix = String::new();
    if let Some(stripped) = rest.strip_prefix('[') {
        if let Some(close) = stripped.find(']') {
            if LogLevel::parse(&stripped[..close]).is_none() {
                prefix.push_str(&rest[..close + 2]);
                prefix.push(' ');
                rest = stripped[close + 1..].trim_start();
            }
        }
    }

    let mut level = None;
    rest = strip_separators(rest);
    if let Some((token, after)) = take_token(rest) {
        if let Some(parsed) = LogLevel::parse(token) {
            level = Some(parsed);
            rest = strip_separators(after);
        }
    }

    if at.is_none() && level.is_none() {
        return None;
    }
    let message = format!("{prefix}{rest}");
    let message = message.trim_end();
    Some(ParsedLine {
        level,
        message: (!message.is_empty()).then(|| message.to_string()),
        fields: BTreeMap::new(),
        at,
    })
}

fn take_token(input: &str) -> Option<(&str, &str)> {
    let skip = input.find(|c: char| !c.is_whitespace())?;
    let rest = &input[skip..];
    match rest.find(char::is_whitespace) {
        Some(end) => Some((&rest[..end], rest[end..].trim_start())),
        None => Some((rest, "")),
    }
}

fn trim_token_punctuation(token: &str) -> &str {
    token.trim_end_matches([',', ';', '|', ']'])
}

fn strip_separators(input: &str) -> &str {
    input.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '-' | ':' | '|' | '=' | '>' | '[' | ']' | ',')
    })
}

/// 常见「日期 + 时间」字面量；带 `%z` 的由 chrono 自行处理时区偏移。
const TIMESTAMP_FORMATS: [&str; 7] = [
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y/%m/%d %H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%d/%b/%Y:%H:%M:%S",
    "%Y-%m-%d %H:%M",
    "%Y-%m-%d %H:%M:%S%.f%z",
    "%Y-%m-%dT%H:%M:%S%.f%z",
];

/// 解析绝对时间：RFC 3339 优先，其次常见「日期 + 时间」字面量（无时区时按本机时区）。
#[must_use]
pub fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.with_timezone(&Utc));
    }
    for format in TIMESTAMP_FORMATS {
        if format.ends_with("%z") {
            if let Ok(dt) = DateTime::parse_from_str(text, format) {
                return Some(dt.with_timezone(&Utc));
            }
            continue;
        }
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(text, format) {
            if let Some(dt) = Local.from_local_datetime(&naive).earliest() {
                return Some(dt.with_timezone(&Utc));
            }
        }
    }
    None
}

/// 数字时间戳：按数量级判定秒 / 毫秒 / 微秒 / 纳秒。
fn parse_epoch(raw: &str) -> Option<DateTime<Utc>> {
    let value: i64 = raw.trim().parse().ok()?;
    let millis = match value.abs() {
        0..=99_999_999_999 => value.checked_mul(1_000)?,
        100_000_000_000..=99_999_999_999_999 => value,
        100_000_000_000_000..=99_999_999_999_999_999 => value / 1_000,
        _ => value / 1_000_000,
    };
    DateTime::from_timestamp_millis(millis)
}
