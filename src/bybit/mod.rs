//! Bybit integration — public REST and public WebSocket.
//!
//! No API keys are required: every endpoint and stream exposed here is
//! unauthenticated. Bybit v5 unifies spot, linear (USDT perpetual), and
//! inverse contracts under one API surface keyed by a `category`
//! parameter — encoded as the [`BybitCategory`] enum.

pub mod rest;
pub mod ws;

pub use rest::{
    BybitCategory, BybitFundingRate, BybitKline, BybitListResult, BybitLongShortRatio,
    BybitOpenInterest, BybitOrderBook, BybitRestClient, BybitTicker, BybitTrade,
};
pub use ws::BybitConnector;
