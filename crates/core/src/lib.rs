pub mod audit;
pub mod cgroup;
pub mod config;
pub mod error;
pub mod health;
pub mod history;
pub mod logs;
pub mod manager;
pub mod notify;
pub mod process;
pub mod sockets;

pub use config::AppConfig;
pub use error::{GuguError, Result};
pub use manager::SharedManager;

#[cfg(test)]
mod tests {
    mod cgroup;
    mod config;
    mod logs;
    mod manager;
    mod process;
    pub(crate) mod support;
}
