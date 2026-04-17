use gugu_core::manager::SharedManager;

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
