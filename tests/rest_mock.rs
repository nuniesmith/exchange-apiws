//! REST integration tests — exercises `get_balance`, `place_order`, envelope
//! unwrapping, `calc_contracts`, `close_position`, `cancel_order`,
//! `get_position`, `get_recent_fills`, and error paths (429, 500, guard errors)
//! against a local `wiremock` server so no live credentials or network access
//! are needed.
//!
//! Run with:
//! ```text
//! cargo test --test rest_mock
//! ```

use exchange_apiws::{
    ExchangeError, KuCoinClient,
    client::Credentials,
    rest::MarginModel,
    types::{OrderType, Side},
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a `KuCoinClient` pointed at the wiremock server.
///
/// We use `Credentials::new` directly rather than `Credentials::sim()` because
/// integration tests compile the lib crate without `cfg(test)`, so items gated
/// on `#[cfg(any(test, feature = "testutils"))]` are not visible here.
fn sim_client(base_url: &str) -> KuCoinClient {
    KuCoinClient::with_base_url(
        Credentials::new("sim_key", "sim_secret", "sim_pass"),
        base_url,
    )
    .expect("failed to build test client")
}

/// Minimal KuCoin success envelope wrapping `data`.
fn ok_envelope(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "code": "200000", "data": data })
}

/// KuCoin error envelope (non-200000 code).
fn err_envelope(code: &str, msg: &str) -> serde_json::Value {
    serde_json::json!({ "code": code, "msg": msg })
}

// ── get_balance ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_balance_returns_available_balance() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .and(query_param("currency", "USDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "availableBalance": 1234.56,
                "orderMargin":      0.0,
                "positionMargin":   0.0,
                "unrealisedPNL":    0.0,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let balance = client
        .get_balance("USDT")
        .await
        .expect("get_balance failed");
    assert!(
        (balance - 1234.56).abs() < 1e-9,
        "expected 1234.56, got {balance}"
    );
}

#[tokio::test]
async fn get_balance_zero_is_valid() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "availableBalance": 0.0,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let balance = client
        .get_balance("USDT")
        .await
        .expect("get_balance failed");
    assert!(balance.abs() < f64::EPSILON, "expected 0.0, got {balance}");
}

// ── get_position ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_position_returns_current_qty() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/position"))
        .and(query_param("symbol", "XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":        "XBTUSDTM",
                "currentQty":    10,
                "avgEntryPrice": 71000.0,
                "unrealisedPnl": 150.0,
                "isOpen":        true,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let pos = client
        .get_position("XBTUSDTM")
        .await
        .expect("get_position failed");

    assert!((pos.current_qty - 10.0).abs() < 1e-9);
    assert_eq!(pos.symbol, "XBTUSDTM");
    assert!(pos.is_long());
    assert!(!pos.is_short());
    assert!(!pos.is_flat());
}

#[tokio::test]
async fn get_position_short_returns_negative_qty() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/position"))
        .and(query_param("symbol", "ETHUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "ETHUSDTM",
                "currentQty": -20,
                "isOpen":     true,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let pos = client
        .get_position("ETHUSDTM")
        .await
        .expect("get_position failed");

    assert!((pos.current_qty + 20.0).abs() < 1e-9);
    assert!(pos.is_short());
    assert!(!pos.is_long());
    assert!(!pos.is_flat());
}

#[tokio::test]
async fn get_position_flat_returns_zero_qty() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/position"))
        .and(query_param("symbol", "SOLUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "SOLUSDTM",
                "currentQty": 0,
                "isOpen":     false,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let pos = client
        .get_position("SOLUSDTM")
        .await
        .expect("get_position failed");

    assert!(pos.is_flat());
}

// ── place_order ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn place_market_order_returns_order_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "order-abc-123" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Market,
            None,
            None,
            false,
            None,
        )
        .await
        .expect("place_order failed");

    assert_eq!(resp.order_id, "order-abc-123");
}

#[tokio::test]
async fn place_limit_order_without_price_returns_order_error() {
    let server = MockServer::start().await;
    let client = sim_client(&server.uri());

    let err = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Limit,
            None,
            None,
            false,
            None,
        )
        .await
        .expect_err("should have returned an error for missing limit price");

    assert!(
        matches!(err, ExchangeError::Order(_)),
        "expected Order error, got {err:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn place_limit_order_with_price_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "limit-order-789" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            5,
            10,
            OrderType::Limit,
            Some(70_000.0),
            None,
            false,
            None,
        )
        .await
        .expect("limit order failed");

    assert_eq!(resp.order_id, "limit-order-789");
}

// ── close_position ────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_position_long_sends_sell_order() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "close-long-001" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .close_position("XBTUSDTM", 10, 10)
        .await
        .expect("close_position failed");

    assert_eq!(resp.order_id, "close-long-001");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn close_position_short_sends_buy_order() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "close-short-002" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .close_position("ETHUSDTM", -20, 5)
        .await
        .expect("close_position failed");

    assert_eq!(resp.order_id, "close-short-002");
}

#[tokio::test]
async fn close_position_qty_zero_returns_order_error() {
    let server = MockServer::start().await;
    let client = sim_client(&server.uri());

    let err = client
        .close_position("SOLUSDTM", 0, 5)
        .await
        .expect_err("should have returned an error for qty=0");

    assert!(
        matches!(err, ExchangeError::Order(_)),
        "expected Order error, got {err:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ── cancel_order ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_order_calls_delete_with_order_id() {
    let server = MockServer::start().await;
    let order_id = "abc-order-xyz";

    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/orders/{order_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "cancelledOrderIds": [order_id] }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .cancel_order(order_id)
        .await
        .expect("cancel_order failed");

    assert_eq!(resp["cancelledOrderIds"][0], order_id);
}

#[tokio::test]
async fn cancel_all_orders_calls_delete_returns_id_list() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "cancelledOrderIds": ["id1", "id2"] }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .cancel_all_orders("XBTUSDTM")
        .await
        .expect("cancel_all_orders failed");

    let ids = resp["cancelledOrderIds"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
}

// ── get_open_orders ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_open_orders_returns_items_list() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders"))
        .and(query_param("status", "active"))
        .and(query_param("symbol", "XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "items": [
                    {
                        "id":     "order-1",
                        "symbol": "XBTUSDTM",
                        "side":   "buy",
                        "type":   "limit",
                        "status": "active",
                        "size":   5,
                    },
                    {
                        "id":     "order-2",
                        "symbol": "XBTUSDTM",
                        "side":   "sell",
                        "type":   "market",
                        "status": "active",
                        "size":   3,
                    }
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let orders = client
        .get_open_orders("XBTUSDTM")
        .await
        .expect("get_open_orders failed");

    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].id, "order-1");
    assert!(orders[0].is_active());
    assert_eq!(orders[1].id, "order-2");
}

#[tokio::test]
async fn get_open_orders_empty_returns_empty_vec() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let orders = client
        .get_open_orders("SOLUSDTM")
        .await
        .expect("get_open_orders failed");

    assert!(orders.is_empty());
}

// ── get_recent_fills ──────────────────────────────────────────────────────────

#[tokio::test]
async fn get_recent_fills_returns_fill_list() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/recentFills"))
        .and(query_param("symbol", "SOLUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!([
                {
                    "symbol":  "SOLUSDTM",
                    "orderId": "order-fill-1",
                    "side":    "buy",
                    "price":   82.15,
                    "size":    30,
                    "fee":     0.04,
                    "tradeId": "trade-abc",
                },
                {
                    "symbol":  "SOLUSDTM",
                    "orderId": "order-fill-2",
                    "side":    "sell",
                    "price":   82.35,
                    "size":    30,
                    "fee":     0.04,
                }
            ]))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let fills = client
        .get_recent_fills("SOLUSDTM")
        .await
        .expect("get_recent_fills failed");

    assert_eq!(fills.len(), 2);
    assert_eq!(fills[0].order_id, "order-fill-1");
    assert_eq!(fills[0].side, "buy");
    assert!((fills[0].price - 82.15).abs() < 1e-9);
    assert_eq!(fills[1].side, "sell");
}

// ── Envelope unwrapping ───────────────────────────────────────────────────────

#[tokio::test]
async fn api_error_envelope_surfaces_as_exchange_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("400100", "KC-API-KEY not exists.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .get_balance("USDT")
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, message } => {
            assert_eq!(code, "400100");
            assert!(
                message.contains("KC-API-KEY"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn success_envelope_with_nested_data_deserializes_correctly() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "orderId": "envelope-test-456",
                "unknown": "field",
                "nested":  { "also": "ignored" },
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_order(
            "XBTUSDTM",
            Side::Sell,
            2,
            5,
            OrderType::Market,
            None,
            None,
            true,
            None,
        )
        .await
        .expect("envelope unwrap failed");

    assert_eq!(resp.order_id, "envelope-test-456");
}

// ── HTTP error codes ──────────────────────────────────────────────────────────

#[tokio::test]
async fn http_429_surfaces_as_api_error_after_cap() {
    // Client retries on 429 up to MAX_RATE_LIMIT_RETRIES, then gives up.
    // Set gw-ratelimit-reset=1 (ms) so the test doesn't hang.
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .respond_with(ResponseTemplate::new(429).insert_header("gw-ratelimit-reset", "1"))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .get_balance("USDT")
        .await
        .expect_err("should have errored after rate-limit retries");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "429"),
        other => panic!("expected ExchangeError::Api(429), got {other:?}"),
    }
}

#[tokio::test]
async fn http_500_surfaces_as_exchange_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .get_balance("USDT")
        .await
        .expect_err("should have errored on HTTP 500");

    assert!(
        matches!(
            err,
            ExchangeError::Json(_) | ExchangeError::Api { .. } | ExchangeError::Http(_)
        ),
        "unexpected error variant: {err:?}"
    );
}

// ── calc_contracts ────────────────────────────────────────────────────────────

#[tokio::test]
async fn calc_contracts_uses_runtime_multiplier() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "XBTUSDTM",
                "multiplier": 0.001,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    // balance=10_000, leverage=10, risk=2% → notional=200 USDT
    // notional_per_ct = 86_000 * 0.001 = 86 USDT
    // margin_per_ct   = 86 / 10 = 8.6 USDT
    // raw             = 200 / 8.6 ≈ 23
    let n = client
        .calc_contracts("XBTUSDTM", 86_000.0, 10_000.0, 10, 0.02, 100)
        .await
        .expect("calc_contracts failed");

    assert_eq!(n, 23, "expected 23 contracts, got {n}");
}

#[tokio::test]
async fn calc_contracts_respects_max_contracts_cap() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "XBTUSDTM",
                "multiplier": 0.001,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    // raw ≈ 23, capped at 5
    let n = client
        .calc_contracts("XBTUSDTM", 86_000.0, 10_000.0, 10, 0.02, 5)
        .await
        .expect("calc_contracts failed");

    assert_eq!(n, 5, "expected cap at 5 contracts, got {n}");
}

#[tokio::test]
async fn calc_contracts_floors_at_one_contract() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "XBTUSDTM",
                "multiplier": 0.001,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    // Tiny balance → raw truncates to 0, floor to 1.
    let n = client
        .calc_contracts("XBTUSDTM", 86_000.0, 1.0, 10, 0.01, 100)
        .await
        .expect("calc_contracts failed");

    assert!(n >= 1, "expected at least 1 contract, got {n}");
}

#[tokio::test]
async fn calc_contracts_errors_when_multiplier_is_missing() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/NEWTOKEN"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol": "NEWTOKEN",
                // no "multiplier" field
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .calc_contracts("NEWTOKEN", 1.0, 1000.0, 10, 0.01, 50)
        .await
        .expect_err("should have errored on missing multiplier");

    assert!(
        matches!(err, ExchangeError::Order(_)),
        "expected Order error, got {err:?}"
    );
}

// ── stop orders ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_open_stop_orders_returns_list() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/stopOrders"))
        .and(query_param("symbol", "XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "items": [
                    {
                        "id":         "stop-1",
                        "symbol":     "XBTUSDTM",
                        "side":       "sell",
                        "type":       "market",
                        "stop":       "down",
                        "stopPrice":  85000.0,
                        "size":       5,
                        "reduceOnly": true,
                    },
                    {
                        "id":         "stop-2",
                        "symbol":     "XBTUSDTM",
                        "side":       "buy",
                        "type":       "limit",
                        "stop":       "up",
                        "stopPrice":  92000.0,
                        "price":      92100.0,
                        "size":       3,
                        "reduceOnly": false,
                    },
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let stops = client
        .get_open_stop_orders("XBTUSDTM")
        .await
        .expect("get_open_stop_orders failed");

    assert_eq!(stops.len(), 2);
    assert_eq!(stops[0].id, "stop-1");
    assert_eq!(stops[0].side, "sell");
    assert_eq!(stops[0].order_type, "market");
    assert_eq!(stops[0].stop.as_deref(), Some("down"));
    assert_eq!(stops[0].size, 5);
    assert_eq!(stops[0].reduce_only, Some(true));

    assert_eq!(stops[1].id, "stop-2");
    assert_eq!(stops[1].order_type, "limit");
    assert_eq!(stops[1].stop.as_deref(), Some("up"));
    assert_eq!(stops[1].reduce_only, Some(false));
}

#[tokio::test]
async fn get_open_stop_orders_empty_returns_empty_vec() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/stopOrders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let stops = client
        .get_open_stop_orders("XBTUSDTM")
        .await
        .expect("get_open_stop_orders failed");

    assert!(stops.is_empty());
}

#[tokio::test]
async fn place_stop_market_order_returns_order_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "stop-market-abc" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_stop_order(
            "XBTUSDTM",
            Side::Sell,
            5,
            10,
            85_000.0,
            "down",
            None, // no limit price → stop-market
            true,
        )
        .await
        .expect("place_stop_order failed");

    assert_eq!(resp.order_id, "stop-market-abc");
}

#[tokio::test]
async fn place_stop_limit_order_returns_order_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "orderId": "stop-limit-xyz" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_stop_order(
            "XBTUSDTM",
            Side::Buy,
            3,
            10,
            92_000.0,
            "up",
            Some(92_100.0), // limit price → stop-limit
            false,
        )
        .await
        .expect("place_stop_limit_order failed");

    assert_eq!(resp.order_id, "stop-limit-xyz");
}

#[tokio::test]
async fn cancel_stop_order_returns_exchange_response() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/stopOrders/stop-to-cancel"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "cancelledOrderIds": ["stop-to-cancel"] }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .cancel_stop_order("stop-to-cancel")
        .await
        .expect("cancel_stop_order failed");

    let ids = resp["cancelledOrderIds"]
        .as_array()
        .expect("cancelledOrderIds should be an array");
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].as_str(), Some("stop-to-cancel"));
}

#[tokio::test]
async fn cancel_all_stop_orders_returns_exchange_response() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/stopOrders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "cancelledOrderIds": ["stop-1", "stop-2"] }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .cancel_all_stop_orders("XBTUSDTM")
        .await
        .expect("cancel_all_stop_orders failed");

    let ids = resp["cancelledOrderIds"]
        .as_array()
        .expect("cancelledOrderIds should be an array");
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn cancel_all_stop_orders_no_open_stops_returns_empty() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/stopOrders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "cancelledOrderIds": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .cancel_all_stop_orders("SOLUSDTM")
        .await
        .expect("cancel_all_stop_orders failed");

    let ids = resp["cancelledOrderIds"]
        .as_array()
        .expect("cancelledOrderIds should be an array");
    assert!(ids.is_empty());
}

// ── get_done_orders ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_done_orders_returns_filled_orders() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders"))
        .and(query_param("status", "done"))
        .and(query_param("symbol", "XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "items": [
                    {
                        "id":          "done-order-1",
                        "symbol":      "XBTUSDTM",
                        "side":        "buy",
                        "type":        "market",
                        "status":      "done",
                        "size":        10,
                        "filledSize":  10,
                    },
                    {
                        "id":          "done-order-2",
                        "symbol":      "XBTUSDTM",
                        "side":        "sell",
                        "type":        "limit",
                        "status":      "done",
                        "price":       88000.0,
                        "size":        5,
                        "filledSize":  5,
                    },
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let orders = client
        .get_done_orders("XBTUSDTM", 10)
        .await
        .expect("get_done_orders failed");

    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].id, "done-order-1");
    assert_eq!(orders[0].status, "done");
    assert!(!orders[0].is_active());
    assert!(orders[0].is_filled());

    assert_eq!(orders[1].id, "done-order-2");
    assert_eq!(orders[1].price, Some(88_000.0));
    assert!(orders[1].is_filled());
}

#[tokio::test]
async fn get_done_orders_empty_returns_empty_vec() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders"))
        .and(query_param("status", "done"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let orders = client
        .get_done_orders("SOLUSDTM", 10)
        .await
        .expect("get_done_orders failed");

    assert!(orders.is_empty());
}

// ── get_order (by id) ─────────────────────────────────────────────────────────

#[tokio::test]
async fn get_order_by_id_returns_order_detail() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders/order-lookup-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "id":          "order-lookup-1",
                "symbol":      "XBTUSDTM",
                "side":        "buy",
                "type":        "limit",
                "status":      "active",
                "price":       71000.0,
                "size":        8,
                "filledSize":  0,
                "reduceOnly":  false,
                "leverage":    "10",
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let order = client
        .get_order("order-lookup-1")
        .await
        .expect("get_order failed");

    assert_eq!(order.id, "order-lookup-1");
    assert_eq!(order.symbol, "XBTUSDTM");
    assert_eq!(order.side, "buy");
    assert_eq!(order.price, Some(71_000.0));
    assert_eq!(order.size, 8);
    assert!(order.is_active());
    assert!(!order.is_filled());
}

#[tokio::test]
async fn get_order_by_id_not_found_returns_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/orders/nonexistent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(err_envelope("400100", "order not found")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .get_order("nonexistent")
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "400100"),
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

// ── set_auto_deposit ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_auto_deposit_enable_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeAutoDeposit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .set_auto_deposit("XBTUSDTM", true)
        .await
        .expect("set_auto_deposit(true) failed");
}

#[tokio::test]
async fn set_auto_deposit_disable_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeAutoDeposit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .set_auto_deposit("XBTUSDTM", false)
        .await
        .expect("set_auto_deposit(false) failed");
}

// ── set_risk_limit_level ──────────────────────────────────────────────────────

#[tokio::test]
async fn set_risk_limit_level_api_error_surfaces() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/risk-limit-level/change"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("300013", "Risk limit level invalid.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .set_risk_limit_level("XBTUSDTM", 99)
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "300013"),
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

// ── get_account_overview_all ──────────────────────────────────────────────────

#[tokio::test]
async fn get_account_overview_all_returns_all_currencies() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/account-overview-all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "summary": [
                    {
                        "availableBalance": 5000.0,
                        "orderMargin":      100.0,
                        "positionMargin":   200.0,
                        "unrealisedPNL":    -15.5,
                    },
                    {
                        "availableBalance": 0.25,
                    },
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let overviews = client
        .get_account_overview_all()
        .await
        .expect("get_account_overview_all failed");

    assert_eq!(overviews.len(), 2);
    assert!((overviews[0].available_balance - 5000.0).abs() < 1e-9);
    assert_eq!(overviews[0].order_margin, Some(100.0));
    assert_eq!(overviews[0].position_margin, Some(200.0));
    assert!((overviews[1].available_balance - 0.25).abs() < 1e-9);
    assert_eq!(overviews[1].order_margin, None);
}

#[tokio::test]
async fn get_transfer_list_no_filters_returns_all() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/transfer-list"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let records = client
        .get_transfer_list(None, None, 10)
        .await
        .expect("get_transfer_list(no filters) failed");

    assert!(records.is_empty());
}

// ── set_auto_deposit ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_auto_deposit_true_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeAutoDeposit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .set_auto_deposit("XBTUSDTM", true)
        .await
        .expect("set_auto_deposit failed");
}

#[tokio::test]
async fn set_auto_deposit_false_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeAutoDeposit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .set_auto_deposit("XBTUSDTM", false)
        .await
        .expect("set_auto_deposit(false) failed");
}

#[tokio::test]
async fn set_auto_deposit_api_error_propagates() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeAutoDeposit"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("300010", "Position does not exist.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .set_auto_deposit("NOSUCHSYM", true)
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "300010"),
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

// ── set_risk_limit_level ──────────────────────────────────────────────────────

#[tokio::test]
async fn set_risk_limit_level_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/risk-limit-level/change"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "level": 2 }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .set_risk_limit_level("XBTUSDTM", 2)
        .await
        .expect("set_risk_limit_level failed");
}

#[tokio::test]
async fn set_risk_limit_level_api_error_propagates() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/risk-limit-level/change"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("300011", "Invalid risk limit level.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .set_risk_limit_level("XBTUSDTM", 999)
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "300011"),
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

// ── add_position_margin ───────────────────────────────────────────────────────

#[tokio::test]
async fn add_position_margin_in_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeMargin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .add_position_margin("XBTUSDTM", 50.0, "IN")
        .await
        .expect("add_position_margin IN failed");
}

#[tokio::test]
async fn add_position_margin_out_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/position/changeMargin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({}))))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    client
        .add_position_margin("XBTUSDTM", 25.0, "OUT")
        .await
        .expect("add_position_margin OUT failed");
}

// ── get_account_overview_all ──────────────────────────────────────────────────

#[tokio::test]
async fn get_account_overview_all_returns_multi_currency() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/account-overview-all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "summary": [
                    { "availableBalance": 1000.0, "orderMargin": 50.0,  "positionMargin": 200.0, "unrealisedPNL":  10.0 },
                    { "availableBalance": 0.05,   "orderMargin": 0.0,   "positionMargin": 0.0,   "unrealisedPNL":   0.0 },
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let overviews = client
        .get_account_overview_all()
        .await
        .expect("get_account_overview_all failed");

    assert_eq!(overviews.len(), 2);
    assert!((overviews[0].available_balance - 1000.0).abs() < 1e-9);
    assert_eq!(overviews[0].order_margin, Some(50.0));
    assert!((overviews[1].available_balance - 0.05).abs() < 1e-9);
}

#[tokio::test]
async fn get_account_overview_all_empty_returns_empty_vec() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v2/account-overview-all"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "summary": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let overviews = client
        .get_account_overview_all()
        .await
        .expect("get_account_overview_all failed");

    assert!(overviews.is_empty());
}

// ── transfer_to_main ──────────────────────────────────────────────────────────

#[tokio::test]
async fn transfer_to_main_returns_apply_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/transfer-out"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "applyId": "transfer-out-abc" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .transfer_to_main("USDT", 500.0)
        .await
        .expect("transfer_to_main failed");

    assert_eq!(resp.apply_id, "transfer-out-abc");
}

#[tokio::test]
async fn transfer_to_main_api_error_propagates() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/transfer-out"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("200004", "Insufficient balance.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .transfer_to_main("USDT", 999_999.0)
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, .. } => assert_eq!(code, "200004"),
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

// ── transfer_to_futures ───────────────────────────────────────────────────────

#[tokio::test]
async fn transfer_to_futures_returns_apply_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/transfer-in"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            serde_json::json!({ "applyId": "transfer-in-xyz" }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .transfer_to_futures("USDT", 250.0)
        .await
        .expect("transfer_to_futures failed");

    assert_eq!(resp.apply_id, "transfer-in-xyz");
}

// ── get_transfer_list ─────────────────────────────────────────────────────────

#[tokio::test]
async fn get_transfer_list_returns_records() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/transfer-list"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "items": [
                    {
                        "applyId":  "t-001",
                        "currency": "USDT",
                        "status":   "SUCCESS",
                        "type":     "TRANSFER_OUT",
                        "amount":   500.0,
                    },
                    {
                        "applyId":  "t-002",
                        "currency": "USDT",
                        "status":   "PROCESSING",
                        "type":     "TRANSFER_IN",
                        "amount":   250.0,
                    },
                ]
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let records = client
        .get_transfer_list(Some("USDT"), None, 10)
        .await
        .expect("get_transfer_list failed");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].apply_id.as_deref(), Some("t-001"));
    assert_eq!(records[0].currency, "USDT");
    assert_eq!(records[0].status.as_deref(), Some("SUCCESS"));
    assert_eq!(records[0].transfer_type.as_deref(), Some("TRANSFER_OUT"));
    assert!((records[0].amount.unwrap() - 500.0).abs() < 1e-9);

    assert_eq!(records[1].apply_id.as_deref(), Some("t-002"));
    assert_eq!(records[1].status.as_deref(), Some("PROCESSING"));
    assert_eq!(records[1].transfer_type.as_deref(), Some("TRANSFER_IN"));
}

#[tokio::test]
async fn get_transfer_list_no_filter_returns_all() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/transfer-list"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    // currency=None, transfer_type=None — broadest query
    let records = client
        .get_transfer_list(None, None, 10)
        .await
        .expect("get_transfer_list (no filter) failed");

    assert!(records.is_empty());
}

#[tokio::test]
async fn get_transfer_list_max_count_capped_at_50() {
    // Pass 999 — the implementation caps at 50 before sending.
    // We just need to verify the call succeeds (the cap is server-side enforced,
    // so the mock will match any maxCount value here).
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/transfer-list"))
        .and(query_param("maxCount", "50"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "items": [] }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let records = client
        .get_transfer_list(None, None, 999)
        .await
        .expect("get_transfer_list with capped maxCount failed");

    assert!(records.is_empty());
}

// ── UTA / margin endpoints ────────────────────────────────────────────────────

#[tokio::test]
async fn get_uta_account_summary_parses_fields() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v3/account/summary"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "accountEquityTotal":   "10000.00",
                "unrealisedPNLTotal":   "12.50",
                "marginBalanceTotal":   "9987.50",
                "positionMarginTotal":  "100.00",
                "orderMarginTotal":     "50.00",
                "frozenFundsTotal":     "0.00",
                "availableBalanceTotal":"9837.50",
                "totalCurrency":        "USDT"
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let summary = sim_client(&server.uri())
        .get_uta_account_summary()
        .await
        .expect("uta summary");
    assert!((summary.account_equity_total - 10_000.0).abs() < 1e-6);
    assert!((summary.unrealised_pnl_total - 12.5).abs() < 1e-9);
    assert_eq!(summary.total_currency, "USDT");
}

#[tokio::test]
async fn get_cross_margin_accounts_parses_assets() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v3/margin/accounts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "totalAssetOfQuoteCurrency":    "10000.0",
                "totalLiabilityOfQuoteCurrency": "0.0",
                "debtRatio":                     "0.0",
                "status":                        "EFFECTIVE",
                "assets": [{
                    "currency":         "USDT",
                    "borrowEnabled":    true,
                    "repayEnabled":     true,
                    "transferEnabled":  true,
                    "borrowed":         "0.0",
                    "totalAsset":       "10000.0",
                    "available":        "10000.0",
                    "hold":             "0.0",
                    "maxBorrowSize":    "5000.0"
                }]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let m = sim_client(&server.uri())
        .get_cross_margin_accounts()
        .await
        .expect("cross margin");
    assert_eq!(m.status, "EFFECTIVE");
    assert_eq!(m.assets.len(), 1);
    assert_eq!(m.assets[0].currency, "USDT");
    assert_eq!(m.assets[0].max_borrow_size, Some(5_000.0));
}

#[tokio::test]
async fn get_isolated_margin_accounts_parses_pairs() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/isolated/accounts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "totalConversionBalance":     "1000.0",
                "liabilityConversionBalance": "0.0",
                "assets": [{
                    "symbol":    "BTC-USDT",
                    "status":    "EFFECTIVE",
                    "debtRatio": "0.0",
                    "baseAsset": {
                        "currency":         "BTC",
                        "totalBalance":     "0.01",
                        "holdBalance":      "0.0",
                        "availableBalance": "0.01",
                        "liability":        "0.0",
                        "interest":         "0.0",
                        "borrowableAmount": "0.005",
                        "borrowEnabled":    true,
                        "transferInEnabled":true,
                        "repayEnabled":     true
                    },
                    "quoteAsset": {
                        "currency":         "USDT",
                        "totalBalance":     "500.0",
                        "holdBalance":      "0.0",
                        "availableBalance": "500.0",
                        "liability":        "0.0",
                        "interest":         "0.0",
                        "borrowableAmount": "250.0",
                        "borrowEnabled":    true,
                        "transferInEnabled":true,
                        "repayEnabled":     true
                    }
                }]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let m = sim_client(&server.uri())
        .get_isolated_margin_accounts()
        .await
        .expect("isolated margin");
    assert_eq!(m.total_conversion_balance, Some(1_000.0));
    assert_eq!(m.assets.len(), 1);
    assert_eq!(m.assets[0].symbol, "BTC-USDT");
    assert_eq!(m.assets[0].base_asset.currency, "BTC");
    assert_eq!(m.assets[0].quote_asset.borrowable_amount, Some(250.0));
}

// ── Spot-margin orders ────────────────────────────────────────────────────────

#[tokio::test]
async fn place_margin_order_returns_order_id_and_borrow_size() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/margin/order"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "orderId":     "margin-order-1",
                "borrowSize":  "0.005",
                "loanApplyId": "loan-1"
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let resp = sim_client(&server.uri())
        .place_margin_order(
            "BTC-USDT",
            Side::Buy,
            OrderType::Limit,
            0.01,
            Some(30_000.0),
            MarginModel::Cross,
            true,
            None,
        )
        .await
        .expect("place margin order");
    assert_eq!(resp.order_id, "margin-order-1");
    assert_eq!(resp.borrow_size.as_deref(), Some("0.005"));
    assert_eq!(resp.loan_apply_id.as_deref(), Some("loan-1"));
}

#[tokio::test]
async fn get_margin_order_returns_typed_detail() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/margin/orders/o-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "id":"o-1","symbol":"BTC-USDT","side":"buy","type":"limit",
                "size":"0.01","price":"30000","dealSize":"0","dealFunds":"0",
                "marginModel":"cross","timeInForce":"GTC","isActive":true,
                "cancelExist":false,"createdAt":1_700_000_000_000_u64
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let detail = sim_client(&server.uri())
        .get_margin_order("o-1")
        .await
        .expect("get margin order");
    assert_eq!(detail.id, "o-1");
    assert_eq!(detail.side, "buy");
    assert!(detail.is_active);
    assert!((detail.size_f64() - 0.01).abs() < 1e-9);
}

#[tokio::test]
async fn cancel_margin_order_returns_cancelled_ids() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/margin/orders/o-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "cancelledOrderIds": ["o-1"]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server.uri())
        .cancel_margin_order("o-1")
        .await
        .expect("cancel margin order");
    assert_eq!(r.cancelled_order_ids, vec!["o-1".to_string()]);
}

#[tokio::test]
async fn cancel_margin_order_already_done_returns_empty() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/margin/orders/o-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "cancelledOrderIds": []
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server.uri())
        .cancel_margin_order("o-1")
        .await
        .expect("cancel margin order");
    assert!(r.cancelled_order_ids.is_empty());
}

#[tokio::test]
async fn get_margin_fills_unwraps_items_page() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/margin/fills"))
        .and(query_param("symbol", "BTC-USDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "currentPage": 1, "pageSize": 50, "totalNum": 2, "totalPage": 1,
                "items": [
                    {"symbol":"BTC-USDT","orderId":"o-1","side":"buy","price":"30000","size":"0.005","funds":"150","fee":"0.15","feeCurrency":"USDT","liquidity":"taker","tradeId":"t-1","createdAt":1_700_000_000_000_u64},
                    {"symbol":"BTC-USDT","orderId":"o-2","side":"sell","price":"30010","size":"0.003","funds":"90.03","fee":"0.09","feeCurrency":"USDT","liquidity":"maker","tradeId":"t-2","createdAt":1_700_000_000_050_u64}
                ]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let fills = sim_client(&server.uri())
        .get_margin_fills("BTC-USDT")
        .await
        .expect("get margin fills");
    assert_eq!(fills.len(), 2);
    assert_eq!(fills[0].side, "buy");
    assert_eq!(fills[1].liquidity.as_deref(), Some("maker"));
}

#[tokio::test]
async fn get_margin_balance_v1_parses_accounts() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/margin/account"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "debtRatio": "0",
                "status":    "EFFECTIVE",
                "accounts":  [{
                    "currency":         "USDT",
                    "totalBalance":     "100",
                    "availableBalance": "100",
                    "holdBalance":      "0",
                    "liability":        "0",
                    "maxBorrowSize":    "50"
                }]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let bal = sim_client(&server.uri())
        .get_margin_balance()
        .await
        .expect("get margin balance");
    assert_eq!(bal.status.as_deref(), Some("EFFECTIVE"));
    assert_eq!(bal.accounts.len(), 1);
    assert_eq!(bal.accounts[0].currency, "USDT");
    assert_eq!(bal.accounts[0].max_borrow_size.as_deref(), Some("50"));
}

#[tokio::test]
async fn place_margin_order_propagates_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/margin/order"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("200004", "Balance insufficient")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = sim_client(&server.uri())
        .place_margin_order(
            "BTC-USDT",
            Side::Buy,
            OrderType::Market,
            10.0,
            None,
            MarginModel::Cross,
            false,
            None,
        )
        .await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "200004");
            assert!(message.contains("Balance insufficient"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn uta_account_summary_propagates_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v3/account/summary"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(err_envelope("411100", "UTA not enabled")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = sim_client(&server.uri()).get_uta_account_summary().await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "411100");
            assert!(message.contains("UTA not enabled"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
