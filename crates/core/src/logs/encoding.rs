//! 日志文本解码：子进程输出可能是 UTF-8，也可能是 Windows 本地代码页（GBK）。
//!
//! 落盘的日志始终是原始字节，解码只发生在「进内存、进广播、进检索」这一步：
//! 面板显示的是解码后的文本，下载原始文件仍能对上程序当时的输出。

use serde::{Deserialize, Serialize};

/// 日志文本编码。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogEncoding {
    /// 先按 UTF-8 严格解码，失败再按 GBK（中文 Windows 的代码页 936）解码。
    #[default]
    Auto,
    /// 强制 UTF-8；非法字节用替换字符。
    #[serde(rename = "utf-8", alias = "utf8")]
    Utf8,
    /// 强制 GBK；非法字节用替换字符。
    #[serde(alias = "cp936")]
    Gbk,
}

impl LogEncoding {
    /// 把一行原始字节解成文本；不合法的字节用替换字符，绝不吞掉整行。
    #[must_use]
    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Auto => match std::str::from_utf8(bytes) {
                Ok(text) => text.to_string(),
                Err(_) => decode_gbk(bytes),
            },
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Gbk => decode_gbk(bytes),
        }
    }
}

fn decode_gbk(bytes: &[u8]) -> String {
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    text.into_owned()
}

impl std::str::FromStr for LogEncoding {
    type Err = String;

    /// 解析配置值：`auto` / `utf-8`（`utf8`）/ `gbk`（`cp936`），忽略大小写与首尾空白。
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "utf-8" | "utf8" => Ok(Self::Utf8),
            "gbk" | "cp936" => Ok(Self::Gbk),
            other => Err(format!(
                "未知日志编码 {other}，可选 auto / utf-8（utf8）/ gbk（cp936）"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// “你好，世界” 的 GBK 字节。
    const GBK_HELLO: &[u8] = &[0xC4, 0xE3, 0xBA, 0xC3, 0xA3, 0xAC, 0xCA, 0xC0, 0xBD, 0xE7];

    #[test]
    fn auto_keeps_utf8_and_falls_back_to_gbk() {
        let utf8 = "你好，世界";
        assert_eq!(LogEncoding::Auto.decode(utf8.as_bytes()), utf8);
        assert_eq!(LogEncoding::Auto.decode(GBK_HELLO), utf8);
        assert_eq!(LogEncoding::Auto.decode(b"plain ascii"), "plain ascii");
    }

    #[test]
    fn forced_encodings_do_not_sniff() {
        let utf8 = "你好，世界";
        assert_eq!(LogEncoding::Gbk.decode(GBK_HELLO), utf8);
        assert_eq!(LogEncoding::Utf8.decode(utf8.as_bytes()), utf8);
        assert!(
            LogEncoding::Utf8.decode(GBK_HELLO).contains('\u{FFFD}'),
            "强制 UTF-8 时 GBK 字节只能得到替换字符"
        );
    }

    #[test]
    fn invalid_bytes_never_drop_the_line() {
        // 0xFF 不是任何编码的合法起始字节；后面跟的 0xFE 是 GBK 合法首字节，
        // 会把下一个字节当尾字节吃掉，所以只断言整行没被丢、坏字节有替换字符。
        let mut bytes = vec![b'a'];
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        bytes.push(b'b');
        for encoding in [LogEncoding::Auto, LogEncoding::Utf8, LogEncoding::Gbk] {
            let text = encoding.decode(&bytes);
            assert!(text.starts_with('a'), "{encoding:?}: {text:?}");
            assert!(text.contains('\u{FFFD}'), "{encoding:?}: {text:?}");
        }
        assert_eq!(
            LogEncoding::Utf8.decode(&bytes),
            "a\u{FFFD}\u{FFFD}b",
            "强制 UTF-8 时逐字节替换，内容不丢"
        );
    }

    #[test]
    fn config_values_parse_with_aliases() {
        for (value, expected) in [
            ("auto", LogEncoding::Auto),
            (" UTF8 ", LogEncoding::Utf8),
            ("utf-8", LogEncoding::Utf8),
            ("cp936", LogEncoding::Gbk),
            ("gbk", LogEncoding::Gbk),
        ] {
            assert_eq!(value.parse::<LogEncoding>(), Ok(expected), "{value}");
        }
        assert!("latin1".parse::<LogEncoding>().is_err());
    }
}
