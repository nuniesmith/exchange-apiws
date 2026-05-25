//! Binance integration — public REST and public WebSocket.
//!
//! No API keys are required: every endpoint and stream exposed here is
//! unauthenticated. Authenticated channels (user-data, account, orders)
//! are intentionally out of scope — this crate is a market-data and
//! infrastructure library for exchanges where the user has either no keys
//! or only public access.

pub mod rest;
pub mod ws;

pub use rest::{
    BinanceBookTicker, BinanceFundingRate, BinanceKline, BinanceMarkPrice, BinanceOpenInterest,
    BinanceOrderBook, BinanceRestClient, BinanceTicker24h, BinanceTrade,
};
pub use ws::BinanceConnector;
