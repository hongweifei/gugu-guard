//! 文件管理接口的请求与响应类型。

use serde::{Deserialize, Serialize};

/// 只带路径的查询参数。
#[derive(Debug, Deserialize)]
pub(super) struct PathQuery {
    #[serde(default)]
    pub(super) path: Option<String>,
    /// 文本预览的字节上限；缺省用服务端默认值。
    #[serde(default)]
    pub(super) limit: Option<u64>,
}

/// 上传：目标路径与是否覆盖同名文件。
#[derive(Debug, Deserialize)]
pub(super) struct UploadQuery {
    pub(super) path: String,
    #[serde(default)]
    pub(super) overwrite: bool,
}

/// 创建分片上传会话。
#[derive(Debug, Deserialize)]
pub(super) struct BeginBody {
    /// 目标路径；同时放在查询串里供审计记录对象。
    pub(super) path: String,
    /// 客户端声明的总字节数；0 表示不校验总量。
    #[serde(default)]
    pub(super) size: u64,
    #[serde(default)]
    pub(super) overwrite: bool,
}

/// 追加分片：偏移必须与服务端记录一致。
///
/// 审计对象来自原始查询串里的 `path`，不在这里解析。
#[derive(Debug, Deserialize)]
pub(super) struct ChunkQuery {
    pub(super) offset: u64,
}

/// 分片上传会话的状态快照：续传前先取一次偏移。
#[derive(Debug, Serialize)]
pub(super) struct UploadSession {
    pub(super) id: String,
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) offset: u64,
}

/// 追加分片的结果：服务端已收到的总字节数。
#[derive(Debug, Serialize)]
pub(super) struct ChunkResult {
    pub(super) offset: u64,
}

/// 保存文本。
#[derive(Debug, Deserialize)]
pub(super) struct WriteBody {
    pub(super) path: String,
    pub(super) content: String,
    /// 目标不存在时是否新建。
    #[serde(default)]
    pub(super) create: bool,
}

/// 新建目录。
#[derive(Debug, Deserialize)]
pub(super) struct MkdirBody {
    pub(super) path: String,
    /// 逐级创建，已存在不算错。
    #[serde(default)]
    pub(super) parents: bool,
}

/// 新建空文件。
#[derive(Debug, Deserialize)]
pub(super) struct PathBody {
    pub(super) path: String,
}

/// 改名、移动与复制的共同形状。
#[derive(Debug, Deserialize)]
pub(super) struct PairBody {
    pub(super) from: String,
    pub(super) to: String,
    #[serde(default)]
    pub(super) overwrite: bool,
}

/// 删除。
#[derive(Debug, Deserialize)]
pub(super) struct RemoveQuery {
    pub(super) path: String,
    /// 目录非空时必须显式为 true。
    #[serde(default)]
    pub(super) recursive: bool,
}

/// 文件管理的可用根目录与写权限：前端据此渲染根切换与写按钮。
#[derive(Debug, Serialize)]
pub(super) struct FsRoots {
    pub(super) roots: Vec<String>,
    pub(super) writable: bool,
}

/// 写操作的结果：给前端与 CLI 一句可直接展示的话。
#[derive(Debug, Serialize)]
pub(super) struct FsSuccess {
    pub(super) message: String,
    pub(super) path: String,
}

impl FsSuccess {
    pub(super) fn new(message: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: path.into(),
        }
    }
}
