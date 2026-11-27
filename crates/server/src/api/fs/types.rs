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
