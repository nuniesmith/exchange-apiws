//! WebSocket modules — types, token negotiation, connector, and runner.

mod connect;
pub mod feed;
pub mod runner;
pub mod types;

pub use feed::KucoinConnector;
pub use runner::{
    EventListener, RunnerEvent, SupervisedConfig, WsFeedEndpoint, WsRunnerConfig, run_feed,
    run_feed_supervised,
};
pub use types::*;
