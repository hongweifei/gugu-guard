use std::sync::Arc;

use gugu_core::manager::SharedManager;

use crate::metrics::Metrics;

#[derive(Clone)]
pub struct AppState {
    pub manager: SharedManager,
    pub metrics: Arc<Metrics>,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
}

impl AppState {
    pub fn new(manager: SharedManager, api_key: Option<String>, cors_origins: Vec<String>) -> Self {
        Self {
            manager,
            metrics: Arc::new(Metrics::new()),
            api_key,
            cors_origins,
        }
    }
}
