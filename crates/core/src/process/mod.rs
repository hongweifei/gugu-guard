//! 被守护子进程的管理：类型定义、启动/停止/进程树终止、输出采集。
//!
//! 子模块分工（对外路径由本文件 re-export 保持稳定）：
//! - [`types`]：状态与数据模型（ProcessStatus / ProcessInfo / LogEntry / LogStream）
//! - [`managed`]：`ManagedProcess`（进程生命周期状态机）
//! - [`command`]：命令构造（分词、PATH 解析、shell/直接启动）
//! - [`terminate`]：停止与强杀（停止命令、信号、taskkill / force）
//! - [`capture`]：stdout/stderr 采集与日志文件（含轮转）
//! - [`job`]：(Windows) Job Object 进程树封装

mod capture;
mod command;
#[cfg(windows)]
mod job;
mod managed;
mod terminate;
mod types;

pub use managed::ManagedProcess;
pub use types::{LogEntry, LogStream, ProcessInfo, ProcessStatus};

// 单元测试通过 crate::process::* 访问（保持拆分前的路径不变）
#[cfg(test)]
pub(crate) use capture::read_stream;
#[cfg(test)]
pub(crate) use command::split_command_words;
