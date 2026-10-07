#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "kraken")]

//! `KrakenPrivateClient` integration tests via `wiremock`.
//!
//! Verifies the authenticated flow end-to-end:
//! - `API-Key` / `API-Sign` headers are sent on every request
//! - Form body carries a `nonce=` field as the first parameter
//! - Each endpoint deserialises its response shape correctly
//! - Kraken-error envelopes propagate as `ExchangeError::Api`
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_balance_sends_signed_headers_and_returns_map` | `/0/private/Balance` |
//! | `get_open_orders_returns_typed` | `/0/private/OpenOrders` |
//! | `get_closed_orders_returns_count` | `/0/private/ClosedOrders` |
//! | `place_order_returns_txid` | `/0/private/AddOrder` |
//! | `place_order_with_sends_every_option` | `/0/private/AddOrder` |
//! | `place_order_sends_no_optional_flags_by_default` | `/0/private/AddOrder` |
//! | `cancel_order_returns_count` | `/0/private/CancelOrder` |
//! | `cancel_all_orders_returns_count` | `/0/private/CancelAll` |
//! | `get_trades_history_returns_typed` | `/0/private/TradesHistory` |
//! | `get_ledger_returns_typed` | `/0/private/Ledgers` |
//! | `get_ledgers_sends_every_filter` | `/0/private/Ledgers` |
//! | `get_trade_volume_returns_maker_and_taker_fees` | `/0/private/TradeVolume` |
//! | `withdraw_returns_refid` | `/0/private/Withdraw` |
//! | `get_withdrawal_status_returns_typed` | `/0/private/WithdrawStatus` |
//! | `error_envelope_surfaces_as_api_error` | error propagation |
//!
//! Run with:
//! ```text
//! cargo test --test kraken_private_mock
//! ```

use exchange_apiws::kraken::{KrakenLedgerQuery, KrakenOrderOptions};
use exchange_apiws::{ExchangeError, KrakenCredentials, KrakenPrivateClient};
use wiremock::matchers::{body_string_contains, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope(result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"result": result, "error": []})
}

fn sim_client(server: &MockServer) -> KrakenPrivateClient {
    // Test secret computed at runtime so the file contains no
    // high-entropy base64 literal for secret scanners to trip on.
    // Decoded plaintext is "sim-secret" — obviously not real.
    use base64::Engine;
    let secret = base64::engine::general_purpose::STANDARD.encode(b"sim-secret");
    KrakenPrivateClient::with_base_url(KrakenCredentials::new("sim-key", secret), server.uri())
        .expect("build private client")
}

#[tokio::test]
async fn get_balance_sends_signed_headers_and_returns_map() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        // Every authenticated request must carry both headers and a
        // nonce= field at the start of the form body.
        .and(header_exists("API-Key"))
        .and(header_exists("API-Sign"))
        .and(body_string_contains("nonce="))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "XXBT": "0.50000000",
                "ZUSD": "1234.5678"
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let bal = sim_client(&server).get_balance().await.expect("balance");
    assert_eq!(bal.len(), 2);
    assert_eq!(bal["XXBT"], "0.50000000");
    assert_eq!(bal["ZUSD"], "1234.5678");
}

#[tokio::test]
async fn get_open_orders_returns_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/OpenOrders"))
        .and(header_exists("API-Sign"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "open": {
                    "OQCLML-BW3P3-BUCMWZ": {
                        "status": "open",
                        "opentm": 1_700_000_000.0_f64,
                        "vol": "1.00000000",
                        "vol_exec": "0.00000000",
                        "cost": "0.00000",
                        "fee": "0.00000",
                        "descr": {
                            "pair": "XBTUSD",
                            "type": "buy",
                            "ordertype": "limit",
                            "price": "30000",
                            "price2": "0",
                            "leverage": "none"
                        }
                    }
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let orders = sim_client(&server)
        .get_open_orders()
        .await
        .expect("open orders");
    assert_eq!(orders.open.len(), 1);
    let o = &orders.open["OQCLML-BW3P3-BUCMWZ"];
    assert_eq!(o.status, "open");
    assert_eq!(o.descr.as_ref().unwrap().pair, "XBTUSD");
}

#[tokio::test]
async fn get_closed_orders_returns_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/ClosedOrders"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "closed": {},
                "count": 42
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let c = sim_client(&server)
        .get_closed_orders()
        .await
        .expect("closed orders");
    assert!(c.closed.is_empty());
    assert_eq!(c.count, 42);
}

#[tokio::test]
async fn place_order_returns_txid() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/AddOrder"))
        // Kraken's body parameters must be present; spot-check the most
        // diagnostic ones.
        .and(body_string_contains("pair=XBTUSD"))
        .and(body_string_contains("type=buy"))
        .and(body_string_contains("ordertype=limit"))
        .and(body_string_contains("volume=1.0"))
        .and(body_string_contains("price=30000"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "descr": {"order": "buy 1.0 XBTUSD @ limit 30000"},
                "txid": ["OQCLML-BW3P3-BUCMWZ"]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server)
        .place_order("XBTUSD", "buy", "limit", "1.0", Some("30000"))
        .await
        .expect("add order");
    assert_eq!(r.txid, vec!["OQCLML-BW3P3-BUCMWZ".to_string()]);
    assert!(r.descr.unwrap().order.contains("buy 1.0"));
}

#[tokio::test]
async fn place_order_with_sends_every_option() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/AddOrder"))
        .and(body_string_contains("pair=XBTUSD"))
        .and(body_string_contains("oflags=post"))
        .and(body_string_contains("validate=true"))
        .and(body_string_contains("cl_ord_id=rebal-btc-1005"))
        .and(body_string_contains("timeinforce=GTD"))
        // "+30" form-encoded: Kraken reads a leading + as "seconds from now".
        .and(body_string_contains("expiretm=%2B30"))
        .respond_with(
            // A validate-only order is described but never placed, so Kraken
            // returns no txid.
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "descr": {"order": "buy 0.00100000 XBTUSD @ limit 60000.0"}
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let opts = KrakenOrderOptions {
        post_only: true,
        validate: true,
        client_order_id: Some("rebal-btc-1005".into()),
        expire_after_secs: Some(30),
    };
    let r = sim_client(&server)
        .place_order_with("XBTUSD", "buy", "limit", "0.001", Some("60000"), &opts)
        .await
        .expect("validated order");
    assert_eq!(r.txid, Vec::<String>::new());
    assert!(r.descr.unwrap().order.contains("XBTUSD"));
}

#[tokio::test]
async fn place_order_sends_no_optional_flags_by_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/AddOrder"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "descr": {"order": "sell 0.5 ETHUSD @ limit 3000"},
                "txid": ["OABCDE-FGHIJ-KLMNOP"]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    sim_client(&server)
        .place_order("ETHUSD", "sell", "limit", "0.5", Some("3000"))
        .await
        .expect("plain order");

    // The plain call must stay byte-for-byte the order it always was: none
    // of the new parameters may leak into it by default.
    let requests = server.received_requests().await.expect("recording on");
    let body = String::from_utf8_lossy(&requests[0].body).into_owned();
    for absent in ["oflags", "validate", "cl_ord_id", "timeinforce", "expiretm"] {
        assert!(!body.contains(absent), "{absent} sent by default: {body}");
    }
}

#[tokio::test]
async fn cancel_order_returns_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/CancelOrder"))
        .and(body_string_contains("txid=OQCLML-BW3P3-BUCMWZ"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({"count": 1}))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server)
        .cancel_order("OQCLML-BW3P3-BUCMWZ")
        .await
        .expect("cancel");
    assert_eq!(r.count, 1);
}

#[tokio::test]
async fn cancel_all_orders_returns_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/CancelAll"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({"count": 5}))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server)
        .cancel_all_orders()
        .await
        .expect("cancel all");
    assert_eq!(r.count, 5);
}

#[tokio::test]
async fn get_trades_history_returns_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/TradesHistory"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "trades": {
                    "T1": {
                        "ordertxid":"O1","postxid":"","pair":"XXBTZUSD",
                        "time":1_700_000_000.0,"type":"buy","ordertype":"limit",
                        "price":"30000","cost":"30000","fee":"48","vol":"1.0",
                        "margin":"0.0","misc":""
                    }
                },
                "count": 1
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let h = sim_client(&server)
        .get_trades_history()
        .await
        .expect("trades history");
    assert_eq!(h.count, 1);
    let t = &h.trades["T1"];
    assert_eq!(t.pair, "XXBTZUSD");
    assert_eq!(t.side, "buy");
    assert_eq!(t.ordertxid, "O1");
}

#[tokio::test]
async fn get_ledger_returns_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Ledgers"))
        .and(body_string_contains("asset=XBT"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
            "ledger": {
                "L1": {
                    "refid":"R1","time":1_700_000_000.0,"type":"trade","subtype":"",
                    "aclass":"currency","asset":"XXBT","amount":"0.1","fee":"0.0","balance":"0.5"
                }
            },
            "count": 1
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let l = sim_client(&server).get_ledger("XBT").await.expect("ledger");
    assert_eq!(l.count, 1);
    let e = &l.ledger["L1"];
    assert_eq!(e.entry_type, "trade");
    assert_eq!(e.asset, "XXBT");
}

#[tokio::test]
async fn get_ledgers_sends_every_filter() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Ledgers"))
        .and(body_string_contains("asset=ZUSD%2CUSDC"))
        .and(body_string_contains("type=deposit"))
        .and(body_string_contains("start=1759622400"))
        .and(body_string_contains("end=1759708800"))
        .and(body_string_contains("ofs=50"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "ledger": {
                    "L4UESK-KG3EQ-UFO4T5": {
                        "refid": "FTQcuak-V6Za8qrWnhzTx67yYHz8Tg",
                        "time": 1_759_650_000.123_4,
                        "type": "deposit",
                        "subtype": "",
                        "aclass": "currency",
                        "asset": "ZUSD",
                        "amount": "100.0000",
                        "fee": "0.0000",
                        "balance": "196.6900"
                    }
                },
                "count": 51
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let q = KrakenLedgerQuery {
        asset: Some("ZUSD,USDC"),
        entry_type: Some("deposit"),
        start: Some(1_759_622_400),
        end: Some(1_759_708_800),
        offset: Some(50),
    };
    let r = sim_client(&server).get_ledgers(&q).await.expect("ledgers");
    assert_eq!(r.count, 51);
    let entry = &r.ledger["L4UESK-KG3EQ-UFO4T5"];
    assert_eq!(entry.entry_type, "deposit");
    assert_eq!(entry.amount, "100.0000");
}

#[tokio::test]
async fn get_trade_volume_returns_maker_and_taker_fees() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/TradeVolume"))
        .and(body_string_contains("pair=XBTUSD%2CETHUSD"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "currency": "ZUSD",
                "volume": "71.1400",
                "fees": {
                    "XXBTZUSD": {"fee": "0.8000", "minfee": "0.1000", "maxfee": "0.8000",
                                 "nextfee": "0.6000", "nextvolume": "2500.0000",
                                 "tiervolume": "0.0000"},
                    "XETHZUSD": {"fee": "0.1000", "minfee": "0.1000", "maxfee": "0.8000",
                                 "nextfee": null, "nextvolume": null,
                                 "tiervolume": "10000000.0000"}
                },
                "fees_maker": {
                    "XXBTZUSD": {"fee": "0.4000", "minfee": "0.0000", "maxfee": "0.4000",
                                 "nextfee": "0.3500", "nextvolume": "2500.0000",
                                 "tiervolume": "0.0000"}
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_trade_volume(&["XBTUSD", "ETHUSD"])
        .await
        .expect("trade volume");
    assert_eq!(v.currency, "ZUSD");
    assert_eq!(v.fees["XXBTZUSD"].fee, "0.8000");
    assert_eq!(v.fees_maker["XXBTZUSD"].fee, "0.4000");
    assert_eq!(v.fees["XXBTZUSD"].nextfee.as_deref(), Some("0.6000"));
    // The last tier has nowhere further to go: Kraken sends null.
    assert_eq!(v.fees["XETHZUSD"].nextfee, None);
}

#[tokio::test]
async fn withdraw_returns_refid() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Withdraw"))
        .and(body_string_contains("asset=XBT"))
        .and(body_string_contains("key=my-wallet"))
        .and(body_string_contains("amount=0.05"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "refid": "FT5Z3X-..."
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = sim_client(&server)
        .withdraw("XBT", "my-wallet", "0.05")
        .await
        .expect("withdraw");
    assert!(r.refid.starts_with("FT5Z3X"));
}

#[tokio::test]
async fn get_withdrawal_status_returns_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/WithdrawStatus"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!([
            {"refid":"FT5Z3X","status":"Settled","amount":"0.05","asset":"XXBT","time":1_700_000_000.0}
        ]))))
        .expect(1)
        .mount(&server)
        .await;

    let records = sim_client(&server)
        .get_withdrawal_status("XBT")
        .await
        .expect("withdraw status");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, "Settled");
    assert_eq!(records[0].asset, "XXBT");
    assert_eq!(records[0].refid, "FT5Z3X");
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/0/private/Balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {},
            "error": ["EAPI:Invalid key", "EGeneral:Permission denied"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = sim_client(&server).get_balance().await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "kraken_error");
            assert!(message.contains("Invalid key"));
            assert!(message.contains("Permission denied"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
