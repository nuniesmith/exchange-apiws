//! Bybit integration — public REST/WebSocket plus a signed private REST client.
//!
//! Public market data needs no keys. The signed surface
//! ([`BybitPrivateClient`] with [`BybitCredentials`]) covers account / order /
//! position endpoints — HMAC-SHA256 request signing per Bybit v5. Bybit unifies
//! spot, linear (USDT perpetual), and inverse contracts under one API keyed by
//! a `category` parameter — encoded as the [`BybitCategory`] enum.

pub mod auth;
pub mod private;
pub mod private_ws;
pub mod rest;
pub mod ws;

pub use auth::{BybitCredentials, DEFAULT_RECV_WINDOW};
pub use private::{
    BybitCoinBalance, BybitOrderAck, BybitOrderRequest, BybitOrderSide, BybitOrderType,
    BybitPrivateClient, BybitTimeInForce, BybitWalletBalance,
};
pub use private_ws::BybitPrivateConnector;
pub use rest::{
    BybitCategory, BybitFundingRate, BybitInstrument, BybitKline, BybitLeverageFilter,
    BybitListResult, BybitLongShortRatio, BybitLotSizeFilter, BybitOpenInterest, BybitOrderBook,
    BybitPriceFilter, BybitRestClient, BybitTicker, BybitTrade,
};
pub use ws::BybitConnector;
