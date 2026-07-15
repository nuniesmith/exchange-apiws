//! Binance integration — public market data, the spot **user-data** stream,
//! and the signed private REST surface.
//!
//! Public REST endpoints and market WebSocket streams need no credentials. The
//! spot user-data stream — [`BinanceUserDataConnector`] over the WS, fed by the
//! [`BinanceUserDataRest`] `listenKey` lifecycle — authenticates with an API
//! key (the listenKey endpoints need no HMAC signature). The **signed** REST
//! surface ([`BinanceSignedRest`], keyed by [`BinanceCredentials`]) HMAC-SHA256
//! signs each request for account/balance reads and order placement.

pub mod auth;
pub mod private_rest;
pub mod private_ws;
pub mod rest;
pub mod signed_rest;
pub mod ws;

pub use auth::BinanceCredentials;
pub use private_rest::BinanceUserDataRest;
pub use private_ws::BinanceUserDataConnector;
pub use rest::{
    BinanceBookTicker, BinanceFundingRate, BinanceKline, BinanceMarkPrice, BinanceOpenInterest,
    BinanceOrderBook, BinanceRestClient, BinanceTicker24h, BinanceTrade,
};
pub use signed_rest::{BinanceAccountInfo, BinanceBalance, BinanceOrderAck, BinanceSignedRest};
pub use ws::BinanceConnector;
