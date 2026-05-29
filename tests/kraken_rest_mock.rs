#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "kraken")]

//! Kraken public REST integration tests via `wiremock`.
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_system_status_returns_typed` | `/0/public/SystemStatus` |
//! | `get_assets_returns_map` | `/0/public/Assets` |
//! | `get_asset_pairs_with_filter_returns_one_pair` | `/0/public/AssetPairs?pair=...` |
//! | `get_asset_pairs_unfiltered_returns_all_pairs` | `/0/public/AssetPairs` |
//! | `get_ticker_returns_keyed_entry` | `/0/public/Ticker` |
//! | `get_orderbook_returns_levels` | `/0/public/Depth` |
//! | `get_ohlc_returns_raw_value` | `/0/public/OHLC` (raw Value) |
//! | `get_recent_trades_returns_raw_value` | `/0/public/Trades` (raw Value) |
//! | `get_spread_returns_raw_value` | `/0/public/Spread` (raw Value) |
//! | `error_envelope_surfaces_as_api_error` | non-empty error array propagation |
//!
//! Run with:
//! ```text
//! cargo test --test kraken_rest_mock
//! ```

use exchange_apiws::{ExchangeError, KrakenRestClient};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope(result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"result": result, "error": []})
}

fn client_for(server: &MockServer) -> KrakenRestClient {
    KrakenRestClient::with_base_url(server.uri()).expect("build kraken client")
}

#[tokio::test]
async fn get_system_status_returns_typed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/SystemStatus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({"status":"online","timestamp":"2026-05-25T00:00:00Z"}),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let s = client_for(&server)
        .get_system_status()
        .await
        .expect("system status");
    assert_eq!(s.status, "online");
    assert!(s.timestamp.starts_with("2026-"));
}

#[tokio::test]
async fn get_assets_returns_map() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Assets"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBT": {
                    "aclass":"currency","altname":"XBT","decimals":10,"display_decimals":5,
                    "collateral_value":1.0,"status":"enabled"
                },
                "ZUSD": {
                    "aclass":"currency","altname":"USD","decimals":4,"display_decimals":2
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let assets = client_for(&server).get_assets().await.expect("assets");
    assert_eq!(assets.len(), 2);
    assert_eq!(assets["XXBT"].altname, "XBT");
    assert_eq!(assets["XXBT"].collateral_value, Some(1.0));
    assert!(assets["ZUSD"].collateral_value.is_none());
}

#[tokio::test]
async fn get_asset_pairs_with_filter_returns_one_pair() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/AssetPairs"))
        .and(query_param("pair", "XBTUSD"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBTZUSD": {
                    "altname":"XBTUSD","wsname":"XBT/USD",
                    "base":"XXBT","quote":"ZUSD",
                    "pair_decimals":1,"lot_decimals":8,"lot_multiplier":1,
                    "status":"online"
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let pairs = client_for(&server)
        .get_asset_pairs(Some("XBTUSD"))
        .await
        .expect("asset pairs");
    assert_eq!(pairs.len(), 1);
    let p = &pairs["XXBTZUSD"];
    assert_eq!(p.altname, "XBTUSD");
    assert_eq!(p.wsname.as_deref(), Some("XBT/USD"));
    assert_eq!(p.base, "XXBT");
}

#[tokio::test]
async fn get_asset_pairs_unfiltered_returns_all_pairs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/AssetPairs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBTZUSD": {
                    "altname":"XBTUSD","base":"XXBT","quote":"ZUSD",
                    "pair_decimals":1,"lot_decimals":8,"lot_multiplier":1
                },
                "XETHZUSD": {
                    "altname":"ETHUSD","base":"XETH","quote":"ZUSD",
                    "pair_decimals":2,"lot_decimals":8,"lot_multiplier":1
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let pairs = client_for(&server)
        .get_asset_pairs(None)
        .await
        .expect("asset pairs");
    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains_key("XXBTZUSD"));
    assert!(pairs.contains_key("XETHZUSD"));
}

#[tokio::test]
async fn get_ticker_returns_keyed_entry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Ticker"))
        .and(query_param("pair", "XBTUSD"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBTZUSD": {
                    "a": ["96001.0", "1", "1.000"],
                    "b": ["95999.0", "1", "1.000"],
                    "c": ["96000.0", "0.01"],
                    "v": ["10.5", "100.5"],
                    "p": ["95950.0", "95800.0"],
                    "t": [100, 1000],
                    "l": ["95500.0", "95000.0"],
                    "h": ["96500.0", "97000.0"],
                    "o": "95750.0"
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tickers = client_for(&server)
        .get_ticker("XBTUSD")
        .await
        .expect("ticker");
    let t = &tickers["XXBTZUSD"];
    assert!((t.ask_price() - 96_001.0).abs() < 1e-9);
    assert!((t.last_price() - 96_000.0).abs() < 1e-9);
    assert!((t.volume_24h() - 100.5).abs() < 1e-9);
}

#[tokio::test]
async fn get_orderbook_returns_levels() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Depth"))
        .and(query_param("pair", "XBTUSD"))
        .and(query_param("count", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
            "XXBTZUSD": {
                "asks": [["96000.0","1.5",1_700_000_000_u64], ["96001.0","2.0",1_700_000_000_u64]],
                "bids": [["95999.0","1.0",1_700_000_000_u64], ["95998.0","3.0",1_700_000_000_u64]]
            }
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let books = client_for(&server)
        .get_orderbook("XBTUSD", 5)
        .await
        .expect("depth");
    let book = &books["XXBTZUSD"];
    let asks = book.asks_f64();
    let bids = book.bids_f64();
    assert_eq!(asks.len(), 2);
    assert!((asks[0][0] - 96_000.0).abs() < 1e-9);
    assert!((bids[1][1] - 3.0).abs() < 1e-9);
}

#[tokio::test]
async fn get_ohlc_returns_raw_value() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/OHLC"))
        .and(query_param("pair", "XBTUSD"))
        .and(query_param("interval", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
            "XXBTZUSD": [
                [1_700_000_000_u64, "96000.0", "96100.0", "95900.0", "96050.0", "96025.0", "10.5", 100]
            ],
            "last": 1_700_000_060_u64
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = client_for(&server)
        .get_ohlc("XBTUSD", 1)
        .await
        .expect("ohlc");
    assert_eq!(v["last"], 1_700_000_060_u64);
    assert_eq!(v["XXBTZUSD"][0][1], "96000.0");
}

#[tokio::test]
async fn get_recent_trades_returns_raw_value() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Trades"))
        .and(query_param("pair", "XBTUSD"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBTZUSD": [
                    ["96000.0","0.001",1_700_000_000.123_f64,"b","l",""]
                ],
                "last": "1700000060123456789"
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let v = client_for(&server)
        .get_recent_trades("XBTUSD")
        .await
        .expect("trades");
    assert_eq!(v["XXBTZUSD"][0][0], "96000.0");
    assert_eq!(v["last"], "1700000060123456789");
}

#[tokio::test]
async fn get_spread_returns_raw_value() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Spread"))
        .and(query_param("pair", "XBTUSD"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBTZUSD": [[1_700_000_000_u64, "95999.0", "96001.0"]],
                "last": 1_700_000_060_u64
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let v = client_for(&server)
        .get_spread("XBTUSD")
        .await
        .expect("spread");
    assert_eq!(v["XXBTZUSD"][0][1], "95999.0");
    assert_eq!(v["XXBTZUSD"][0][2], "96001.0");
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/0/public/Ticker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {},
            "error": ["EQuery:Unknown asset pair"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = client_for(&server).get_ticker("NOPE").await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "kraken_error");
            assert!(message.contains("Unknown asset pair"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
