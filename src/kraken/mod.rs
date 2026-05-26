//! Kraken integration — public REST, private REST (signed), public WS.
//!
//! Kraken's API splits cleanly into an **unauthenticated public** side
//! (market data, system status — [`KrakenRestClient`] +
//! [`KrakenConnector::public`]) and an **authenticated private** side
//! (trading, account, withdrawals — [`KrakenPrivateClient`]; private
//! WS channels go through [`KrakenConnector::private`] with a token
//! obtained from `POST /0/private/GetWebSocketsToken`). The private
//! REST side uses HMAC-SHA512 signing implemented in [`auth`].

pub mod auth;
pub mod private;
pub mod rest;
pub mod ws;

pub use auth::{KrakenCredentials, form_encode, sign_kraken_request};
pub use private::{
    KrakenAddOrderResponse, KrakenCancelResponse, KrakenClosedOrders, KrakenOpenOrders,
    KrakenOrder, KrakenOrderDescr, KrakenPrivateClient, KrakenWithdrawResponse,
};
pub use rest::{
    KrakenAsset, KrakenAssetPair, KrakenOrderBook, KrakenRestClient, KrakenSystemStatus,
    KrakenTicker, unwrap_kraken_envelope,
};
pub use ws::KrakenConnector;
