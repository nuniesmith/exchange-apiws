//! Binance public REST integration tests via `wiremock`.
//!
//! Each test stands up a mock HTTP server and exercises one endpoint
//! end-to-end so the response-shape decoding is verified against realistic
//! Binance payloads.
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_klines_returns_parsed_bars` | `GET /api/v3/klines` |
//! | `get_orderbook_returns_levels` | `GET /api/v3/depth` |
//! | `get_recent_trades_returns_list` | `GET /api/v3/trades` |
//! | `get_ticker_returns_book_ticker` | `GET /api/v3/ticker/bookTicker` |
//! | `get_ticker_24h_returns_window_stats` | `GET /api/v3/ticker/24hr` |
//! | `get_exchange_info_returns_raw_json` | `GET /api/v3/exchangeInfo` |
//! | `get_futures_klines_returns_parsed_bars` | `GET /fapi/v1/klines` |
//! | `get_futures_funding_rate_returns_history` | `GET /fapi/v1/fundingRate` |
//! | `get_futures_mark_price_returns_snapshot` | `GET /fapi/v1/premiumIndex` |
//! | `get_futures_open_interest_returns_snapshot` | `GET /fapi/v1/openInterest` |
//!
//! Run with:
//! ```text
//! cargo test --test binance_rest_mock
//! ```

use exchange_apiws::BinanceRestClient;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a client whose spot AND futures base URLs point at the same
/// mock server. Each test runs one server and exercises one endpoint, so
/// there's no ambiguity about which URL was hit.
fn client_pointing_at(server: &MockServer) -> BinanceRestClient {
    BinanceRestClient::with_base_urls(server.uri(), server.uri())
        .expect("failed to build Binance client")
}

// ── Spot ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_klines_returns_parsed_bars() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/klines"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("interval", "1m"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            [
                1_700_000_000_000_u64,
                "96000.10",
                "96100.50",
                "95950.00",
                "96050.25",
                "12.345",
                1_700_000_059_999_u64,
                "1185000.0",
                42_u32,
                "6.0",
                "576000.0",
                "0"
            ],
            [
                1_700_000_060_000_u64,
                "96050.25",
                "96200.00",
                "96000.00",
                "96150.00",
                "8.5",
                1_700_000_119_999_u64,
                "817000.0",
                30_u32,
                "4.0",
                "384000.0",
                "0"
            ]
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let bars = client_pointing_at(&server)
        .get_klines("BTCUSDT", "1m", 2)
        .await
        .expect("expected successful klines fetch");

    assert_eq!(bars.len(), 2);
    assert!((bars[0].open - 96_000.1).abs() < 1e-6);
    assert!((bars[1].close - 96_150.0).abs() < 1e-6);
    assert_eq!(bars[0].trades, 42);
}

#[tokio::test]
async fn get_orderbook_returns_levels() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/depth"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "5"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "lastUpdateId": 1_234_567_890_u64,
                "bids": [["96000.00", "1.5"], ["95999.50", "2.0"]],
                "asks": [["96000.50", "0.8"], ["96001.00", "3.0"]]
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let book = client_pointing_at(&server)
        .get_orderbook("BTCUSDT", 5)
        .await
        .expect("expected successful orderbook fetch");

    assert_eq!(book.last_update_id, 1_234_567_890);
    let bids = book.bids_f64();
    let asks = book.asks_f64();
    assert!((bids[0][0] - 96_000.0).abs() < 1e-9);
    assert!((asks[0][0] - 96_000.5).abs() < 1e-9);
}

#[tokio::test]
async fn get_recent_trades_returns_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/trades"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": 100,
                "price": "96000.00",
                "qty": "0.1",
                "quoteQty": "9600.00",
                "time": 1_700_000_000_000_u64,
                "isBuyerMaker": false,
                "isBestMatch": true
            },
            {
                "id": 101,
                "price": "96005.00",
                "qty": "0.2",
                "quoteQty": "19201.00",
                "time": 1_700_000_000_500_u64,
                "isBuyerMaker": true,
                "isBestMatch": true
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let trades = client_pointing_at(&server)
        .get_recent_trades("BTCUSDT", 2)
        .await
        .expect("expected successful trades fetch");

    assert_eq!(trades.len(), 2);
    assert!((trades[0].price - 96_000.0).abs() < 1e-9);
    assert!(trades[1].is_buyer_maker);
}

#[tokio::test]
async fn get_ticker_returns_book_ticker() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/ticker/bookTicker"))
        .and(query_param("symbol", "BTCUSDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "symbol": "BTCUSDT",
                "bidPrice": "96000.10",
                "bidQty": "1.5",
                "askPrice": "96001.00",
                "askQty": "0.8"
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let t = client_pointing_at(&server)
        .get_ticker("BTCUSDT")
        .await
        .expect("expected successful ticker fetch");
    assert_eq!(t.symbol, "BTCUSDT");
    assert!((t.bid_price - 96_000.1).abs() < 1e-9);
}

#[tokio::test]
async fn get_ticker_24h_returns_window_stats() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/ticker/24hr"))
        .and(query_param("symbol", "BTCUSDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "symbol": "BTCUSDT",
                "priceChange": "500.00",
                "priceChangePercent": "0.52",
                "weightedAvgPrice": "96100.00",
                "lastPrice": "96250.00",
                "lastQty": "0.05",
                "bidPrice": "96249.50",
                "bidQty": "1.0",
                "askPrice": "96250.50",
                "askQty": "1.2",
                "openPrice": "95750.00",
                "highPrice": "96400.00",
                "lowPrice": "95500.00",
                "volume": "1500.5",
                "quoteVolume": "144000000.0",
                "openTime": 1_700_000_000_000_u64,
                "closeTime": 1_700_086_400_000_u64,
                "firstId": 1_000_000_u64,
                "lastId": 1_050_000_u64,
                "count": 50_000_u64
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let t = client_pointing_at(&server)
        .get_ticker_24h("BTCUSDT")
        .await
        .expect("expected successful 24h ticker fetch");
    assert_eq!(t.symbol, "BTCUSDT");
    assert!((t.price_change_percent - 0.52).abs() < 1e-9);
    assert_eq!(t.count, 50_000);
}

#[tokio::test]
async fn get_exchange_info_returns_raw_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/exchangeInfo"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "timezone": "UTC",
                "serverTime": 1_700_000_000_000_u64,
                "rateLimits": [],
                "symbols": []
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let info = client_pointing_at(&server)
        .get_exchange_info()
        .await
        .expect("expected successful exchangeInfo fetch");
    assert_eq!(info["timezone"], "UTC");
    assert_eq!(info["serverTime"], 1_700_000_000_000_u64);
}

// ── Futures (USDT-M) ──────────────────────────────────────────────────────────

#[tokio::test]
async fn get_futures_klines_returns_parsed_bars() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fapi/v1/klines"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("interval", "1m"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            [
                1_700_000_000_000_u64,
                "96000.0",
                "96100.0",
                "95900.0",
                "96050.0",
                "100.5",
                1_700_000_059_999_u64,
                "9650000.0",
                250_u32,
                "50.0",
                "4800000.0",
                "0"
            ]
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let bars = client_pointing_at(&server)
        .get_futures_klines("BTCUSDT", "1m", 1)
        .await
        .expect("expected successful futures klines fetch");
    assert_eq!(bars.len(), 1);
    assert!((bars[0].close - 96_050.0).abs() < 1e-9);
}

#[tokio::test]
async fn get_futures_funding_rate_returns_history() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fapi/v1/fundingRate"))
        .and(query_param("symbol", "BTCUSDT"))
        .and(query_param("limit", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "symbol": "BTCUSDT",
                "fundingRate": "0.0001",
                "fundingTime": 1_700_028_800_000_u64,
                "markPrice": "96010.0"
            },
            {
                "symbol": "BTCUSDT",
                "fundingRate": "-0.00005",
                "fundingTime": 1_700_057_600_000_u64,
                "markPrice": "96020.0"
            },
            {
                "symbol": "BTCUSDT",
                "fundingRate": "0.0",
                "fundingTime": 1_700_086_400_000_u64,
                "markPrice": ""
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let rates = client_pointing_at(&server)
        .get_futures_funding_rate("BTCUSDT", 3)
        .await
        .expect("expected successful funding-rate fetch");

    assert_eq!(rates.len(), 3);
    assert!((rates[0].funding_rate - 0.0001).abs() < 1e-9);
    assert!(rates[1].funding_rate < 0.0);
    // Third row sent markPrice as empty string — should round-trip to None.
    assert!(rates[2].mark_price.is_none());
}

#[tokio::test]
async fn get_futures_mark_price_returns_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fapi/v1/premiumIndex"))
        .and(query_param("symbol", "BTCUSDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "symbol": "BTCUSDT",
                "markPrice": "96010.5",
                "indexPrice": "96005.0",
                "estimatedSettlePrice": "96012.0",
                "lastFundingRate": "0.0001",
                "interestRate": "0.0001",
                "nextFundingTime": 1_700_028_800_000_u64,
                "time": 1_700_000_000_000_u64
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mp = client_pointing_at(&server)
        .get_futures_mark_price("BTCUSDT")
        .await
        .expect("expected successful mark-price fetch");

    assert_eq!(mp.symbol, "BTCUSDT");
    assert!((mp.mark_price - 96_010.5).abs() < 1e-9);
    assert_eq!(mp.next_funding_time, 1_700_028_800_000);
}

#[tokio::test]
async fn get_futures_open_interest_returns_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fapi/v1/openInterest"))
        .and(query_param("symbol", "BTCUSDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "symbol": "BTCUSDT",
                "openInterest": "12345.678",
                "time": 1_700_000_000_000_u64
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let oi = client_pointing_at(&server)
        .get_futures_open_interest("BTCUSDT")
        .await
        .expect("expected successful open-interest fetch");

    assert_eq!(oi.symbol, "BTCUSDT");
    assert!((oi.open_interest - 12_345.678).abs() < 1e-6);
}
