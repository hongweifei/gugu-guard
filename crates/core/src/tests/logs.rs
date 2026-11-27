//! 日志解析、检索条件与历史文件扫描的口径测试。

use chrono::Utc;

use crate::logs::{clean, parse_line, LogFormat, LogLevel, LogQuery, ParsedLine};
use crate::process::{LogEntry, LogStream};
use crate::tests::support::tempdir;

fn entry(stream: LogStream, level: Option<LogLevel>, line: &str) -> LogEntry {
    LogEntry {
        timestamp: Utc::now(),
        stream,
        line: line.to_string(),
        process_name: None,
        parsed: level.map(|level| ParsedLine {
            level: Some(level),
            ..Default::default()
        }),
    }
}

mod parse_json {
    use super::*;

    #[test]
    fn extracts_level_message_time_and_nested_fields() {
        let line = r#"{"time":"2026-11-26T10:00:00Z","level":"warn","msg":"disk 90% full","path":"/var","ctx":{"pid":42}}"#;
        let parsed = parse_line(line, LogFormat::Auto);

        assert_eq!(parsed.level, Some(LogLevel::Warn), "应取 level 字段");
        assert_eq!(
            parsed.message.as_deref(),
            Some("disk 90% full"),
            "应取 msg 字段作为正文"
        );
        assert!(parsed.at.is_some(), "应解析出行内时间");
        assert_eq!(
            parsed.fields.get("path").map(String::as_str),
            Some("/var"),
            "其余键应留在字段里"
        );
        assert_eq!(
            parsed.fields.get("ctx.pid").map(String::as_str),
            Some("42"),
            "嵌套对象应按点号展平"
        );
        assert!(
            !parsed.fields.contains_key("msg"),
            "已提升为正文的键不再重复成字段"
        );
    }

    #[test]
    fn accepts_numeric_levels_from_bunyan_and_syslog() {
        let bunyan = parse_line(r#"{"level":30,"msg":"ok"}"#, LogFormat::Auto);
        assert_eq!(bunyan.level, Some(LogLevel::Info), "30 是 bunyan 的 info");

        let syslog = parse_line(r#"{"level":3,"msg":"bad"}"#, LogFormat::Json);
        assert_eq!(syslog.level, Some(LogLevel::Error), "3 是 syslog 的 err");
    }

    #[test]
    fn parses_epoch_millis_timestamp() {
        let parsed = parse_line(r#"{"ts":1764144000000,"msg":"x"}"#, LogFormat::Auto);
        let at = parsed.at.expect("毫秒时间戳应可解析");
        assert_eq!(at.timestamp(), 1_764_144_000, "毫秒应换算成秒");
    }

    #[test]
    fn ignores_non_json_line_when_declared_json() {
        // 声明 json 的项目偶尔也会打出普通行：解析不出结构化信息，行本身照常保留。
        let parsed = parse_line("plain text from a library", LogFormat::Json);
        assert!(parsed.is_empty(), "纯文本行不应有结构化信息");
    }
}

mod parse_logfmt {
    use super::*;

    #[test]
    fn extracts_pairs_level_and_message() {
        let line = "time=2026-11-26T10:00:00Z level=warn msg=\"disk 90% full\" path=/var";
        let parsed = parse_line(line, LogFormat::Auto);

        assert_eq!(parsed.level, Some(LogLevel::Warn));
        assert_eq!(parsed.message.as_deref(), Some("disk 90% full"));
        assert!(parsed.at.is_some());
        assert_eq!(
            parsed.fields.get("path").map(String::as_str),
            Some("/var"),
            "非提升键应保留为字段"
        );
    }

    #[test]
    fn handles_quoted_escapes() {
        let parsed = parse_line(r#"level=info msg="say \"hi\"" step=1"#, LogFormat::Logfmt);
        assert_eq!(parsed.message.as_deref(), Some("say \"hi\""));
        assert_eq!(parsed.fields.get("step").map(String::as_str), Some("1"));
    }

    #[test]
    fn auto_mode_requires_two_pairs_or_known_key() {
        // 单条 `PATH=...` 更像普通文本，自动模式不应误判成 logfmt。
        let parsed = parse_line("PATH=C:\\bin", LogFormat::Auto);
        assert!(parsed.is_empty(), "单键普通文本不应判定为 logfmt");

        // 声明为 logfmt 时按声明执行。
        let declared = parse_line("PATH=C:\\bin", LogFormat::Logfmt);
        assert_eq!(
            declared.fields.get("PATH").map(String::as_str),
            Some("C:\\bin")
        );
    }
}

mod parse_text {
    use super::*;

    #[test]
    fn reads_timestamp_and_level_prefix() {
        let parsed = parse_line("2026-11-26 10:00:00 INFO starting", LogFormat::Auto);
        assert_eq!(parsed.level, Some(LogLevel::Info));
        assert_eq!(parsed.message.as_deref(), Some("starting"));
        assert!(parsed.at.is_some(), "行首时间应被识别");
        assert!(parsed.fields.is_empty(), "文本行没有结构化字段");
    }

    #[test]
    fn reads_bracketed_timestamp_and_bracketed_level() {
        let parsed = parse_line("[2026-11-26 10:00:00] [ERROR] boom", LogFormat::Auto);
        assert_eq!(parsed.level, Some(LogLevel::Error));
        assert_eq!(parsed.message.as_deref(), Some("boom"));
        assert!(parsed.at.is_some());
    }

    #[test]
    fn time_only_prefix_does_not_invent_date() {
        // log4j 常见的「只有时分秒」前缀：级别能认出来，但不编造日期。
        let parsed = parse_line("09:00:00.123 [main] WARN cache miss", LogFormat::Text);
        assert_eq!(parsed.level, Some(LogLevel::Warn));
        assert_eq!(
            parsed.message.as_deref(),
            Some("[main] cache miss"),
            "线程标记保留在正文里，不丢信息"
        );
        assert!(parsed.at.is_none(), "缺日期时不应给出时间");
    }

    #[test]
    fn plain_line_has_no_structure() {
        let parsed = parse_line("just some output", LogFormat::Text);
        assert!(parsed.is_empty());
    }
}

mod clean_ansi {
    use super::*;

    #[test]
    fn strips_color_sequences_and_control_chars() {
        let raw = "\u{1b}[31merror\u{1b}[0m: boom\u{7}";
        assert_eq!(clean(raw), "error: boom", "颜色与响铃都应去掉");
    }

    #[test]
    fn keeps_tabs_as_spaces_but_drops_other_controls() {
        assert_eq!(clean("a\tb\u{0}c"), "a bc");
        assert_eq!(clean("trailing   "), "trailing");
    }
}

mod level_ranking {
    use super::*;

    #[test]
    fn parses_common_tokens() {
        assert_eq!(LogLevel::parse("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("[warning]"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("Fatal:"), Some(LogLevel::Fatal));
        assert_eq!(LogLevel::parse("50"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("not-a-level"), None);
    }

    #[test]
    fn orders_by_severity() {
        assert!(LogLevel::Warn > LogLevel::Info);
        assert!(LogLevel::Fatal > LogLevel::Error);
        assert!(LogLevel::Trace < LogLevel::Debug);
    }
}

mod query_matching {
    use super::*;

    #[test]
    fn filters_by_min_level() {
        let query = LogQuery {
            min_level: Some(LogLevel::Warn),
            ..Default::default()
        };
        assert!(query.matches(&entry(LogStream::Stdout, Some(LogLevel::Warn), "w")));
        assert!(!query.matches(&entry(LogStream::Stdout, Some(LogLevel::Info), "i")));
        assert!(
            !query.matches(&entry(LogStream::Stdout, None, "no level")),
            "没有级别的行不应冒充 warn"
        );
    }

    #[test]
    fn filters_by_stream_and_all_keywords() {
        let query = LogQuery {
            stream: Some(LogStream::Stderr),
            keywords: LogQuery::keywords_from(Some("Disk  FULL")),
            ..Default::default()
        };
        assert!(query.matches(&entry(LogStream::Stderr, None, "disk space full")));
        assert!(!query.matches(&entry(LogStream::Stdout, None, "disk full")));
        assert!(!query.matches(&entry(LogStream::Stderr, None, "disk has room")));
    }

    #[test]
    fn keywords_are_all_required() {
        let query = LogQuery {
            keywords: LogQuery::keywords_from(Some("a b")),
            ..Default::default()
        };
        assert!(query.matches(&entry(LogStream::Stdout, None, "A and B")));
        assert!(!query.matches(&entry(LogStream::Stdout, None, "only a")));
    }
}

mod search_files {
    use super::*;
    use crate::logs::{search_files, SearchOptions};

    fn write_log(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("写入测试日志失败");
        path
    }

    #[test]
    fn filters_hits_and_reports_scan_stats() {
        let dir = tempdir();
        let path = write_log(
            dir.path(),
            "out.log",
            concat!(
                r#"{"time":"2026-11-26T10:00:00Z","level":"info","msg":"start"}"#,
                "\n",
                r#"{"time":"2026-11-26T10:00:01Z","level":"error","msg":"boom","code":500}"#,
                "\n",
                r#"{"time":"2026-11-26T10:00:02Z","level":"warn","msg":"slow","ms":120}"#,
                "\n",
            ),
        );
        let query = LogQuery {
            min_level: Some(LogLevel::Warn),
            ..Default::default()
        };

        let report = search_files(
            &[(LogStream::Stdout, path)],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &query,
            SearchOptions::new(4096, 10),
        );

        assert_eq!(report.matched, 2, "warn 及以上应命中 2 条");
        assert_eq!(report.hits.len(), 2);
        assert!(!report.truncated);
        assert_eq!(report.scanned_lines, 3);
        assert_eq!(
            report.hits[0]
                .parsed
                .as_ref()
                .and_then(|p| p.message.as_deref()),
            Some("boom"),
            "命中记录应带结构化正文"
        );
        assert_eq!(report.files.len(), 1, "应报告实际扫描的文件");
    }

    #[test]
    fn keeps_newest_hits_when_limit_is_exceeded() {
        let dir = tempdir();
        let path = write_log(
            dir.path(),
            "out.log",
            concat!(
                r#"{"time":"2026-11-26T10:00:00Z","level":"error","msg":"one"}"#,
                "\n",
                r#"{"time":"2026-11-26T10:00:01Z","level":"error","msg":"two"}"#,
                "\n",
            ),
        );
        let query = LogQuery {
            min_level: Some(LogLevel::Error),
            limit: 1,
            ..Default::default()
        };

        let report = search_files(
            &[(LogStream::Stdout, path)],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &query,
            SearchOptions::new(4096, 1),
        );

        assert_eq!(report.matched, 2);
        assert_eq!(report.hits.len(), 1, "超出上限只保留最近的一批");
        assert!(report.truncated, "被截断要如实标记");
        assert_eq!(
            report.hits[0]
                .parsed
                .as_ref()
                .and_then(|p| p.message.as_deref()),
            Some("two")
        );
    }

    #[test]
    fn budget_starts_mid_file_and_drops_partial_line() {
        let dir = tempdir();
        let content = "line-one-marker\nline-two-marker\nline-three-marker\n";
        let path = write_log(dir.path(), "out.log", content);
        let query = LogQuery::default();

        let full_len = content.len() as u64;
        // 从文件中间读起：第一段一定是半行，必须丢掉。
        let report = search_files(
            &[(LogStream::Stdout, path)],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &query,
            SearchOptions::new(full_len - 30, 10),
        );

        assert!(report.truncated, "只读了尾部要标记为截断");
        assert!(
            report.hits.iter().all(|hit| hit.line.ends_with("marker")),
            "半行不应作为完整日志返回，实际: {:?}",
            report.hits.iter().map(|hit| &hit.line).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_streams_keep_scan_order_when_a_timestamp_is_missing() {
        let dir = tempdir();
        let out = write_log(
            dir.path(),
            "out.log",
            r#"{"time":"2026-11-26T10:00:01Z","level":"info","msg":"later"}"#,
        );
        let err = write_log(dir.path(), "err.log", "plain stderr line\n");
        let report = search_files(
            &[(LogStream::Stdout, out), (LogStream::Stderr, err)],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &LogQuery::default(),
            SearchOptions::default(),
        );
        assert_eq!(report.hits.len(), 2, "两条流各命中一条");
        assert_eq!(
            report.hits[0].stream,
            LogStream::Stdout,
            "缺时间时不重排，保持先 stdout 后 stderr"
        );
        assert_eq!(
            report.hits[1].stream,
            LogStream::Stderr,
            "stderr 命中排在 stdout 之后"
        );
    }

    #[test]
    fn lines_without_timestamps_keep_file_order() {
        let dir = tempdir();
        let path = write_log(
            dir.path(),
            "out.log",
            "first plain line\nsecond plain line\nthird plain line\n",
        );
        let report = search_files(
            &[(LogStream::Stdout, path)],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &LogQuery::default(),
            SearchOptions::default(),
        );
        let lines: Vec<&str> = report.hits.iter().map(|hit| hit.line.as_str()).collect();
        assert_eq!(
            lines,
            vec!["first plain line", "second plain line", "third plain line"],
            "没有时间戳的行应按文件原顺序返回"
        );
        assert!(report.hits.iter().all(|hit| hit.at().is_none()));
    }

    #[test]
    fn missing_file_is_skipped_instead_of_failing() {
        let dir = tempdir();
        let report = search_files(
            &[(LogStream::Stderr, dir.path().join("absent.log"))],
            LogFormat::Auto,
            crate::logs::LogEncoding::Auto,
            &LogQuery::default(),
            SearchOptions::default(),
        );
        assert!(report.hits.is_empty());
        assert!(report.files.is_empty(), "不存在的文件不计入扫描范围");
    }
}
