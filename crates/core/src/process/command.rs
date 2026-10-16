//! 命令构造：分词、Windows PATH/PATHEXT 解析、创建标志。

use std::path::{Path, PathBuf};

// CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
#[cfg(windows)]
pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
pub(super) const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// 把命令串按空白分词 (双引号保护带空白的段)，返回 (exe, 前置参数)。
/// 不支持 `\"` 转义；需要复杂引号时用 args 数组或 shell = true。
pub(crate) fn split_command_words(s: &str) -> (String, Vec<String>) {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has_token = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                has_token = true;
            }
            c if (c == ' ' || c == '\t') && !in_quote => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return (String::new(), Vec::new());
    }
    let exe = tokens.remove(0);
    (exe, tokens)
}

#[cfg(windows)]
pub(super) fn is_bat_cmd(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
}

/// Windows: 在 PATH 中按 PATHEXT 解析裸命令名，返回找到的完整路径。
/// 含路径分隔符或找不到的情况返回 None (交由 CreateProcess 自行处理/报错)。
#[cfg(windows)]
pub(super) fn resolve_program(exe: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(exe);
    if exe.contains('\\') || exe.contains('/') {
        return direct.is_file().then_some(direct);
    }
    let path_var = std::env::var_os("PATH")?;
    let has_ext = Path::new(exe).extension().is_some();
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(str::to_lowercase)
        .collect();
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(exe);
        if cand.is_file() {
            return Some(cand);
        }
        if !has_ext {
            for ext in &exts {
                let cand = dir.join(format!("{exe}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}
