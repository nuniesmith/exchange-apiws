//! Coinbase integration — public WebSocket market data (Advanced Trade).
//!
//! A public-feed connector: ticker, market trades, and level2 order book over
//! `wss://advanced-trade-ws.coinbase.com`. Signed REST can follow the
//! established per-exchange pattern if needed.

pub mod ws;

pub use ws::{CoinbaseChannel, CoinbaseConnector};
