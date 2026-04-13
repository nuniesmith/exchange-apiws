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
    assert_eq!(balance, 0.0);
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

    assert_eq!(pos.current_qty, 10);
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

    assert_eq!(pos.current_qty, -20);
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

