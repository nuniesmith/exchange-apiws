#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "cryptocom")]

//! Crypto.com public REST integration tests via `wiremock`.
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_instruments_returns_list` | `/public/get-instruments` |
//! | `get_orderbook_unwraps_single_book` | `/public/get-book` |
//! | `get_candlestick_returns_list` | `/public/get-candlestick` |
//! | `get_ticker_with_instrument_returns_one` | `/public/get-ticker?instrument_name=...` |
//! | `get_ticker_unfiltered_returns_all` | `/public/get-ticker` (no params) |
//! | `get_recent_trades_returns_list` | `/public/get-trades` |
//! | `get_valuations_returns_time_series` | `/public/get-valuations` |
//! | `error_envelope_surfaces_as_api_error` | non-zero `code` propagation |
//! | `orderbook_with_empty_data_array_errors` | edge case — empty response |
//!
//! Run with:
//! ```text
//! cargo test --test cryptocom_rest_mock
//! ```

use exchange_apiws::{CryptocomRestClient, ExchangeError};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope(result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": -1,
        "method": "public/...",
        "code": 0,
        "result": result,
    })
}

fn client_for(server: &MockServer) -> CryptocomRestClient {
    CryptocomRestClient::with_base_url(server.uri()).expect("build")
}

#[tokio::test]
async fn get_instruments_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-instruments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "data": [
                    {
                        "symbol": "BTC_USDT",
                        "inst_type": "CCY_PAIR",
                        "display_name": "BTC/USDT",
                        "base_ccy": "BTC",
                        "quote_ccy": "USDT",
                        "quote_decimals": 2,
                        "quantity_decimals": 8,
                        "price_tick_size": "0.01",
                        "qty_tick_size": "0.00000001",
                        "max_leverage": "10",
                        "tradable": true
                    },
                    {
                        "symbol": "ETH_USDT",
                        "tradable": false
                    }
                ]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let v = client_for(&server)
        .get_instruments()
        .await
        .expect("instruments");
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].symbol, "BTC_USDT");
    assert_eq!(v[0].max_leverage.as_deref(), Some("10"));
    assert_eq!(v[1].symbol, "ETH_USDT");
    assert!(!v[1].tradable);
}

#[tokio::test]
async fn get_orderbook_unwraps_single_book() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-book"))
        .and(query_param("instrument_name", "BTC_USDT"))
        .and(query_param("depth", "10"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "data": [{
                    "instrument_name": "BTC_USDT",
                    "depth": 10,
                    "bids": [["96000.0","1.5","2"], ["95999.0","2.0","3"]],
                    "asks": [["96001.0","0.5","1"]],
                    "timestamp": 1_700_000_000_000_u64,
                    "sequence": 42
                }]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let book = client_for(&server)
        .get_orderbook("BTC_USDT", 10)
        .await
        .expect("book");
    assert_eq!(book.instrument_name, "BTC_USDT");
    assert_eq!(book.sequence, 42);
    let bids = book.bids_f64();
    let asks = book.asks_f64();
    assert!((bids[0][0] - 96_000.0).abs() < 1e-9);
    assert!((asks[0][0] - 96_001.0).abs() < 1e-9);
}

#[tokio::test]
async fn get_candlestick_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-candlestick"))
        .and(query_param("instrument_name", "BTC_USDT"))
        .and(query_param("timeframe", "1m"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
            "instrument_name": "BTC_USDT",
            "interval": "1m",
            "data": [
                {"o":"96000.0","h":"96100.0","l":"95900.0","c":"96050.0","v":"10.5","t":1_700_000_000_000_u64},
                {"o":"96050.0","h":"96200.0","l":"96000.0","c":"96150.0","v":"8.5","t":1_700_000_060_000_u64}
            ]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let bars = client_for(&server)
        .get_candlestick("BTC_USDT", "1m")
        .await
        .expect("candlestick");
    assert_eq!(bars.len(), 2);
    assert!((bars[0].close_f64() - 96_050.0).abs() < 1e-9);
    assert!((bars[1].open_f64() - 96_050.0).abs() < 1e-9);
}

#[tokio::test]
async fn get_ticker_with_instrument_returns_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-ticker"))
        .and(query_param("instrument_name", "BTC_USDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "data": [{
                    "i":"BTC_USDT","a":"96000.0","h":"96500.0","l":"95500.0",
                    "v":"100.5","vv":"9650000","c":"0.005",
                    "b":"95999.0","k":"96001.0","t":1_700_000_000_000_u64
                }]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tickers = client_for(&server)
        .get_ticker(Some("BTC_USDT"))
        .await
        .expect("ticker");
    assert_eq!(tickers.len(), 1);
    assert_eq!(tickers[0].instrument, "BTC_USDT");
    assert_eq!(tickers[0].best_bid.as_deref(), Some("95999.0"));
    assert_eq!(tickers[0].best_ask.as_deref(), Some("96001.0"));
}

#[tokio::test]
async fn get_ticker_unfiltered_returns_all() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-ticker"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "data": [
                    {"i":"BTC_USDT","a":"96000.0","t":1_700_000_000_000_u64},
                    {"i":"ETH_USDT","a":"3200.0","t":1_700_000_000_000_u64}
                ]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tickers = client_for(&server)
        .get_ticker(None)
        .await
        .expect("ticker all");
    assert_eq!(tickers.len(), 2);
    let symbols: Vec<&str> = tickers.iter().map(|t| t.instrument.as_str()).collect();
    assert!(symbols.contains(&"BTC_USDT"));
    assert!(symbols.contains(&"ETH_USDT"));
}

#[tokio::test]
async fn get_recent_trades_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-trades"))
        .and(query_param("instrument_name", "BTC_USDT"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
            "data": [
                {"s":"buy","p":"96000.0","q":"0.05","t":1_700_000_000_000_u64,"d":"t1","i":"BTC_USDT"},
                {"s":"sell","p":"96005.0","q":"0.10","t":1_700_000_000_500_u64,"d":"t2","i":"BTC_USDT"}
            ]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let trades = client_for(&server)
        .get_recent_trades("BTC_USDT")
        .await
        .expect("trades");
    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].side, "buy");
    assert_eq!(trades[1].side, "sell");
    assert_eq!(trades[0].trade_id.as_deref(), Some("t1"));
}

#[tokio::test]
async fn get_valuations_returns_time_series() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-valuations"))
        .and(query_param("instrument_name", "BTCUSD-PERP"))
        .and(query_param("valuation_type", "mark_price"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "instrument_name": "BTCUSD-PERP",
                "valuation_type": "mark_price",
                "data": [
                    {"v":"96010.0","t":1_700_000_000_000_u64},
                    {"v":"96015.0","t":1_700_000_001_000_u64}
                ]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let series = client_for(&server)
        .get_valuations("BTCUSD-PERP", "mark_price")
        .await
        .expect("valuations");
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].value, "96010.0");
    assert!(series[1].timestamp > series[0].timestamp);
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-book"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": -1,
            "method": "public/get-book",
            "code": 30009,
            "message": "Invalid instrument_name",
            "result": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = client_for(&server).get_orderbook("NOPE", 10).await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "30009");
            assert!(message.contains("Invalid instrument_name"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn orderbook_with_empty_data_array_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public/get-book"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "data": []
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = client_for(&server).get_orderbook("BTC_USDT", 10).await;
    match result {
        Err(ExchangeError::Api { code, .. }) => assert_eq!(code, "empty_data"),
        other => panic!("expected Api(empty_data) error, got {other:?}"),
    }
}
