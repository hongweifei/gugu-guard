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
    #[error("循环依赖检测: {0}")]
    CyclicDependency(String),
}

pub type Result<T> = std::result::Result<T, GuguError>;
