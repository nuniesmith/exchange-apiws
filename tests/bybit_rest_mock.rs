#![cfg(feature = "bybit")]

//! Bybit public REST integration tests via `wiremock`.
//!
//! One test per endpoint plus an error-envelope test so the
//! `unwrap_bybit_envelope` path is exercised end-to-end.
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_klines_returns_parsed_bars` | `GET /v5/market/kline` |
//! | `get_orderbook_returns_levels` | `GET /v5/market/orderbook` |
//! | `get_tickers_returns_list` | `GET /v5/market/tickers` |
//! | `get_recent_trades_returns_list` | `GET /v5/market/recent-trade` |
//! | `get_instruments_returns_raw_json` | `GET /v5/market/instruments-info` |
//! | `get_funding_rate_returns_history` | `GET /v5/market/funding/history` |
//! | `get_open_interest_returns_series` | `GET /v5/market/open-interest` |
//! | `get_long_short_ratio_returns_series` | `GET /v5/market/account-ratio` |
//! | `error_envelope_surfaces_as_api_error` | non-zero `retCode` propagation |
//!
//! Run with:
//! ```text
//! cargo test --test bybit_rest_mock
//! ```

use exchange_apiws::{BybitCategory, BybitRestClient, ExchangeError};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope(result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "retCode": 0,
        "retMsg": "OK",
        "result": result,
        "time": 1_700_000_000_000_u64,
    })
}

fn client_for(server: &MockServer) -> BybitRestClient {
    BybitRestClient::with_base_url(server.uri()).expect("build bybit client")
}

#[tokio::test]
async fn get_klines_returns_parsed_bars() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/kline"))
        .and(query_param("category", "linear"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("interval", "1"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [
                    ["1700000000000", "96000.00", "96100.50", "95950.00", "96050.25", "12.345", "1185000.0"],
                    ["1700000060000", "96050.25", "96200.00", "96000.00", "96150.00", "8.5", "817000.0"]
                ]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let result = client_for(&server)
        .get_klines(BybitCategory::Linear, "BTCUSDT", "1", 2)
        .await
        .expect("klines");
    assert_eq!(result.category, "linear");
    assert_eq!(result.list.len(), 2);
    assert!((result.list[0].open - 96_000.0).abs() < 1e-6);
    assert!((result.list[1].close - 96_150.0).abs() < 1e-6);
}

#[tokio::test]
async fn get_orderbook_returns_levels() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/orderbook"))
        .and(query_param("category", "spot"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "s": "BTCUSDT",
                "b": [["96000.00", "1.5"], ["95999.50", "2.0"]],
                "a": [["96000.50", "0.8"], ["96001.00", "3.0"]],
                "ts": 1_700_000_000_000_u64,
                "u": 1_234_567_890_u64
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let book = client_for(&server)
        .get_orderbook(BybitCategory::Spot, "BTCUSDT", 5)
        .await
        .expect("orderbook");
    assert_eq!(book.symbol, "BTCUSDT");
    assert_eq!(book.update_id, 1_234_567_890);
    let bids = book.bids_f64();
    assert!((bids[0][0] - 96_000.0).abs() < 1e-9);
}

#[tokio::test]
async fn get_tickers_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/tickers"))
        .and(query_param("category", "linear"))
        .and(query_param("symbol", "BTCUSDT"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [{
                    "symbol": "BTCUSDT",
                    "lastPrice": "96000.0",
                    "bid1Price": "95999.0",
                    "bid1Size": "1.0",
                    "ask1Price": "96001.0",
                    "ask1Size": "1.5",
                    "highPrice24h": "97000.0",
                    "lowPrice24h": "95000.0",
                    "volume24h": "1500.5",
                    "turnover24h": "144000000.0",
                    "markPrice": "96010.0",
                    "indexPrice": "96005.0",
                    "fundingRate": "0.0001",
                    "nextFundingTime": "1700028800000"
                }]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let tickers = client_for(&server)
        .get_tickers(BybitCategory::Linear, Some("BTCUSDT"))
        .await
        .expect("tickers");
    assert_eq!(tickers.list.len(), 1);
    let t = &tickers.list[0];
    assert_eq!(t.symbol, "BTCUSDT");
    assert!((t.last_price - 96_000.0).abs() < 1e-9);
    assert_eq!(t.mark_price, Some(96_010.0));
    assert_eq!(t.funding_rate, Some(0.0001));
}

#[tokio::test]
async fn get_recent_trades_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/recent-trade"))
        .and(query_param("category", "spot"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "spot",
                "list": [
                    {
                        "execId": "abc",
                        "symbol": "BTCUSDT",
                        "price": "96000.00",
                        "size": "0.1",
                        "side": "Buy",
                        "time": "1700000000000"
                    },
                    {
                        "execId": "def",
                        "symbol": "BTCUSDT",
                        "price": "96005.00",
                        "size": "0.2",
                        "side": "Sell",
                        "time": "1700000000500"
                    }
                ]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let trades = client_for(&server)
        .get_recent_trades(BybitCategory::Spot, "BTCUSDT", 2)
        .await
        .expect("trades");
    assert_eq!(trades.list.len(), 2);
    assert!((trades.list[0].price - 96_000.0).abs() < 1e-9);
    assert_eq!(trades.list[1].side, "Sell");
    assert_eq!(trades.list[1].time, 1_700_000_000_500);
}

#[tokio::test]
async fn get_instruments_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/instruments-info"))
        .and(query_param("category", "linear"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [{"symbol": "BTCUSDT", "status": "Trading"}]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let v = client_for(&server)
        .get_instruments(BybitCategory::Linear)
        .await
        .expect("instruments");
    assert_eq!(v["category"], "linear");
    assert_eq!(v["list"][0]["symbol"], "BTCUSDT");
}

#[tokio::test]
async fn get_funding_rate_returns_history() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/funding/history"))
        .and(query_param("category", "linear"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [
                    {"symbol": "BTCUSDT", "fundingRate": "0.0001", "fundingRateTimestamp": "1700028800000"},
                    {"symbol": "BTCUSDT", "fundingRate": "-0.00005", "fundingRateTimestamp": "1700057600000"}
                ]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let rates = client_for(&server)
        .get_funding_rate(BybitCategory::Linear, "BTCUSDT", 2)
        .await
        .expect("funding history");
    assert_eq!(rates.list.len(), 2);
    assert!(rates.list[1].funding_rate < 0.0);
    assert_eq!(rates.list[0].funding_rate_timestamp, 1_700_028_800_000);
}

#[tokio::test]
async fn get_open_interest_returns_series() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/open-interest"))
        .and(query_param("category", "linear"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("intervalTime", "1h"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [{"openInterest": "12345.678", "timestamp": "1700000000000"}]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let oi = client_for(&server)
        .get_open_interest(BybitCategory::Linear, "BTCUSDT", "1h", 1)
        .await
        .expect("open interest");
    assert_eq!(oi.list.len(), 1);
    assert!((oi.list[0].open_interest - 12_345.678).abs() < 1e-6);
}

#[tokio::test]
async fn get_long_short_ratio_returns_series() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/account-ratio"))
        .and(query_param("category", "linear"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("period", "1h"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({
                "category": "linear",
                "list": [{
                    "symbol": "BTCUSDT",
                    "buyRatio": "0.55",
                    "sellRatio": "0.45",
                    "timestamp": "1700000000000"
                }]
            }),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let ratios = client_for(&server)
        .get_long_short_ratio(BybitCategory::Linear, "BTCUSDT", "1h", 1)
        .await
        .expect("long/short ratio");
    assert_eq!(ratios.list.len(), 1);
    assert!((ratios.list[0].buy_ratio - 0.55).abs() < 1e-9);
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/kline"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "retCode": 10001,
            "retMsg": "invalid symbol",
            "result": {},
            "time": 1_700_000_000_000_u64,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = client_for(&server)
        .get_klines(BybitCategory::Linear, "NOPESYMBOL", "1", 1)
        .await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "10001");
            assert!(message.contains("invalid symbol"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
