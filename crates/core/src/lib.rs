pub mod config;
pub mod error;
pub mod health;
pub mod history;
pub mod manager;
pub mod notify;
pub mod process;
pub mod sockets;

pub use config::AppConfig;
pub use error::{GuguError, Result};
pub use manager::SharedManager;

#[cfg(test)]
mod tests {
    mod config;
    mod manager;
    mod process;
    pub(crate) mod support;
}
