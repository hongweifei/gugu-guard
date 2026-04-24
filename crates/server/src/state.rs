use gugu_core::manager::SharedManager;

/// 应用共享状态，通过 axum State 提取器传递。
#[derive(Clone)]
pub struct AppState {
    pub manager: SharedManager,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
}

impl AppState {
    pub fn new(manager: SharedManager, api_key: Option<String>, cors_origins: Vec<String>) -> Self {
        Self {
            manager,
            api_key,
            cors_origins,
        }
    }
}
