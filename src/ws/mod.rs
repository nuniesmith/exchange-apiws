//! WebSocket modules — types, token negotiation, connector, and runner.

mod connect;
pub mod feed;
pub mod runner;
pub mod types;

pub use feed::KucoinConnector;
pub use runner::{WsRunnerConfig, run_feed};
pub use types::*;
