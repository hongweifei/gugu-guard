pub mod config;
pub mod error;
pub mod health;
pub mod manager;
pub mod process;

pub use config::AppConfig;
pub use error::{GuguError, Result};
pub use manager::SharedManager;

#[cfg(test)]
mod tests {
    mod config;
    mod manager;
    mod process;
}
