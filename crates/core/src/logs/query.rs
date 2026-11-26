//! 日志检索条件：同一份条件同时用于内存环形缓冲过滤与历史文件扫描。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process::{LogEntry, LogStream};

use super::parse::{LogLevel, ParsedLine};

/// 检索条件。所有维度都是「可选 + 同时满足」语义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub stream: Option<LogStream>,
    /// 最低级别（含）：`warn` 只返回 warn/error/fatal。
    #[serde(default)]
    pub min_level: Option<LogLevel>,
    /// 关键字列表，全部命中才算匹配（按子串、忽略 ASCII 大小写）。
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// 返回条数上限（实时取最新的 N 条，文件检索取最后命中的 N 条）。
    #[serde(default = "default_limit")]
    pub limit: usize,
}

const DEFAULT_LIMIT: usize = 100;
const MAX_KEYWORDS: usize = 8;

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            stream: None,
            min_level: None,
            keywords: Vec::new(),
            since: None,
            until: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl LogQuery {
    /// 把用户输入的搜索串拆成关键字：空白分隔、忽略大小写、最多 8 个。
    #[must_use]
    pub fn keywords_from(raw: Option<&str>) -> Vec<String> {
        raw.unwrap_or_default()
            .split_whitespace()
            .take(MAX_KEYWORDS)
            .map(str::to_lowercase)
            .collect()
    }

    /// 条件里除条数上限外还有没有别的维度。
    #[must_use]
    pub fn is_filtering(&self) -> bool {
        self.stream.is_some()
            || self.min_level.is_some()
            || !self.keywords.is_empty()
            || self.since.is_some()
            || self.until.is_some()
    }

    /// 实时日志：时间维度用采集时间（采集时刻是可信的）。
    #[must_use]
    pub fn matches(&self, entry: &LogEntry) -> bool {
        self.matches_parts(
            &entry.stream,
            &entry.line,
            entry.parsed.as_ref(),
            Some(&entry.timestamp),
        )
    }

    /// 通用匹配。`at` 为 None 表示这行没有可信时间（历史文件里解析不出时间戳）：
    /// 一旦按时间过滤就排除，宁可少给结果也不编造时间。
    #[must_use]
    pub fn matches_parts(
        &self,
        stream: &LogStream,
        line: &str,
        parsed: Option<&ParsedLine>,
        at: Option<&DateTime<Utc>>,
    ) -> bool {
        if let Some(want) = &self.stream {
            if want != stream {
                return false;
            }
        }
        if let Some(min) = self.min_level {
            let Some(level) = parsed.and_then(|p| p.level) else {
                return false;
            };
            if level < min {
                return false;
            }
        }
        if self.since.is_some() || self.until.is_some() {
            let Some(at) = at else {
                return false;
            };
            if let Some(since) = &self.since {
                if at < since {
                    return false;
                }
            }
            if let Some(until) = &self.until {
                if at > until {
                    return false;
                }
            }
        }
        if !self.keywords.is_empty() {
            let haystack = line.to_lowercase();
            if !self
                .keywords
                .iter()
                .all(|keyword| haystack.contains(keyword.as_str()))
            {
                return false;
            }
        }
        true
    }
}
