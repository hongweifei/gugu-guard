use gugu_core::manager::SharedManager;

#[derive(Clone)]
pub struct AppState {
    pub manager: SharedManager,
}

impl AppState {
    pub fn new(manager: SharedManager) -> Self {
        Self { manager }
    }
}
