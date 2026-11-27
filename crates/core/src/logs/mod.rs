//! 日志结构化与检索。
//!
//! - [`parse`]：单行解析（JSON / logfmt / 文本启发式），产出级别、正文、字段、行内时间
//! - [`encoding`]：把程序写出的原始字节解成文本（UTF-8 / GBK）
//! - [`query`]：检索条件，实时缓冲与历史文件共用一套匹配语义
//! - [`scan`]：历史日志文件按字节预算倒读检索
//!
//! 一条日志从产生到被检索只有一条链路：采集侧 [`parse_line`] 一次，
//! 历史文件检索侧对落盘行再解析一次，两边解析器相同，因此同一行的级别判定不会漂移。

mod encoding;
mod parse;
mod query;
mod scan;

pub use encoding::LogEncoding;
pub use parse::{clean, parse_line, parse_timestamp, LogFormat, LogLevel, ParsedLine};
pub use query::LogQuery;
pub use scan::{
    search_files, LogSearchReport, SearchHit, SearchOptions, DEFAULT_BUDGET_BYTES, DEFAULT_LIMIT,
    MAX_BUDGET_BYTES,
};
