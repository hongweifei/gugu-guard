//! 文件管理：在允许的根目录内列出、读取、保存、新建、移动、复制与删除。
//!
//! 范围与权限统一由 [`Scope`] 表达；列表与读取不带写权限也能用，
//! 写权限由 `daemon.web.fs_write` 控制，关闭时所有改动入口返回 [`crate::GuguError::ReadOnly`]。

mod entry;
mod guard;
mod ops;
mod text;

pub use entry::{list, stat, Entry, EntryKind, Listing};
pub use guard::Scope;
pub use ops::{copy, create_dir, create_file, relocate, remove};
pub use text::{read, write, TextFile, MAX_PREVIEW_BYTES, MAX_WRITE_BYTES};
