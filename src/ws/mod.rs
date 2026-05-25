//! WebSocket modules — types, token negotiation, connector, runner, and
//! KuCoin's low-latency order-placement client.

mod connect;
pub mod feed;
pub mod orders;
pub mod runner;
pub mod types;

pub use feed::KucoinConnector;
pub use orders::{WsOrderAck, WsOrderClient, build_cancel_order_frame, build_place_order_frame};
pub use runner::{
    EventListener, RunnerEvent, SupervisedConfig, WsFeedEndpoint, WsRunnerConfig, run_feed,
    run_feed_supervised,
};
pub use types::*;
