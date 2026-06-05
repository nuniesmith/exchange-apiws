//! Binance integration — public market data plus the spot **user-data** stream.
//!
//! Public REST endpoints and market WebSocket streams need no credentials. The
//! spot user-data stream — [`BinanceUserDataConnector`] over the WS, fed by the
//! [`BinanceUserDataRest`] `listenKey` lifecycle — authenticates with an API
//! key (the listenKey endpoints need no HMAC signature). Signed account/order
//! REST remains out of scope here.

pub mod private_rest;
pub mod private_ws;
pub mod rest;
pub mod ws;

pub use private_rest::BinanceUserDataRest;
pub use private_ws::BinanceUserDataConnector;
pub use rest::{
    BinanceBookTicker, BinanceFundingRate, BinanceKline, BinanceMarkPrice, BinanceOpenInterest,
    BinanceOrderBook, BinanceRestClient, BinanceTicker24h, BinanceTrade,
};
pub use ws::BinanceConnector;
