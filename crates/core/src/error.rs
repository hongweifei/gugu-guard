use thiserror::Error;

#[derive(Error, Debug)]
pub enum GuguError {
    #[error("进程 '{0}' 未找到")]
    ProcessNotFound(String),
    #[error("进程 '{0}' 已在运行")]
    AlreadyRunning(String),
    #[error("进程 '{0}' 未运行")]
    NotRunning(String),
    #[error("启动进程 '{0}' 失败: {1}")]
    StartFailed(String, String),
    #[error("配置错误: {0}")]
    ConfigError(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("未授权: 无效或缺失 API Key")]
    Unauthorized,
    #[error("路径不在 daemon.web.fs_roots 允许的范围内: {0}")]
    PathDenied(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    InvalidPath(String),
    #[error("文件管理处于只读模式：在 gugu.toml 里把 daemon.web.fs_write 设为 true 才能修改文件")]
    ReadOnly,
    #[error("循环依赖检测: {0}")]
    CyclicDependency(String),
}

pub type Result<T> = std::result::Result<T, GuguError>;
