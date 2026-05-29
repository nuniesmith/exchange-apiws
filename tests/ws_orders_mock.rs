//! `WsOrderClient` integration tests via a local `tokio-tungstenite` server.
//!
//! Exercises the full request/response correlation pipeline without
//! touching live KuCoin:
//!
//! | Test | What it verifies |
//! |------|-----------------|
//! | `place_order_round_trip` | request frame shape + matching `clientOid` ack delivery |
//! | `concurrent_requests_route_by_client_oid` | many in-flight requests resolve to the right futures |
//! | `error_frame_resolves_as_failure` | `"type":"error"` returns `success = false` with code/msg |
//! | `request_times_out_when_no_ack` | server silence triggers the 1 s test timeout |
//! | `close_drops_pending_requests` | `close()` resolves outstanding awaits with `connection_closed` |
//!
//! Run with:
//! ```text
//! cargo test --test ws_orders_mock
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use exchange_apiws::{
    ExchangeError, WsOrderClient,
    types::{OrderType, Side},
};

async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

/// Helper: echo a single ack with the same `clientOid` we received.
fn make_ack(client_oid: &str, order_id: &str) -> String {
    serde_json::json!({
        "id":   "srv-1",
        "type": "ack",
        "data": {"clientOid": client_oid, "orderId": order_id},
    })
    .to_string()
}

#[tokio::test]
async fn place_order_round_trip() {
    let (url, listener) = bind_local().await;

    // Server: read one frame, parse it, send back an ack matching its clientOid.
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        // Drain inbound and ack everything that has a clientOid.
        while let Some(frame) = ws.next().await {
            let Ok(Message::Text(text)) = frame else {
                break;
            };
            let v: Value = serde_json::from_str(text.as_str()).expect("server: json");
            let client_oid = v["data"]["clientOid"]
                .as_str()
                .expect("clientOid")
                .to_string();
            // Echo the inbound type-shape so the test can assert on it too.
            assert_eq!(v["type"], "openOrder");
            assert_eq!(v["data"]["side"], "buy");
            assert_eq!(v["data"]["type"], "limit");
            assert_eq!(v["data"]["size"], 1);
            assert_eq!(v["data"]["price"], "30000");

            ws.send(Message::Text(make_ack(&client_oid, "order-7").into()))
                .await
                .unwrap();
        }
    });

    let client = WsOrderClient::connect(url).await.expect("connect");
    let ack = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Limit,
            Some(30_000.0),
        )
        .await
        .expect("ack");

    assert!(ack.success);
    assert_eq!(ack.order_id.as_deref(), Some("order-7"));
    assert!(!ack.client_oid.is_empty());
    client.close();
}

#[tokio::test]
async fn concurrent_requests_route_by_client_oid() {
    let (url, listener) = bind_local().await;

    // Server: ack every inbound frame, but in REVERSE order to stress the
    // clientOid → oneshot routing (out-of-order arrival).
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let mut collected = Vec::<String>::new();
        for _ in 0..3 {
            let Some(Ok(Message::Text(text))) = ws.next().await else {
                break;
            };
            let v: Value = serde_json::from_str(text.as_str()).unwrap();
            collected.push(v["data"]["clientOid"].as_str().unwrap().to_string());
        }
        // Send acks in reverse order so the first awaiter has to wait for
        // the second/third inbound — this would deadlock without proper
        // map-based routing.
        for (i, oid) in collected.iter().rev().enumerate() {
            ws.send(Message::Text(make_ack(oid, &format!("order-{i}")).into()))
                .await
                .unwrap();
        }
    });

    let client = WsOrderClient::connect(url).await.expect("connect");

    let c1 = client.clone();
    let c2 = client.clone();
    let c3 = client.clone();
    let h1 = tokio::spawn(async move {
        c1.place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Limit,
            Some(30_000.0),
        )
        .await
    });
    let h2 = tokio::spawn(async move {
        c2.place_order(
            "XBTUSDTM",
            Side::Sell,
            1,
            10,
            OrderType::Limit,
            Some(30_010.0),
        )
        .await
    });
    let h3 = tokio::spawn(async move {
        c3.place_order("XBTUSDTM", Side::Buy, 2, 10, OrderType::Market, None)
            .await
    });

    let a1 = h1.await.unwrap().expect("ack 1");
    let a2 = h2.await.unwrap().expect("ack 2");
    let a3 = h3.await.unwrap().expect("ack 3");
    assert!(a1.success && a2.success && a3.success);
    // Each call got its own unique client_oid back (the wrapper assigns one).
    assert_ne!(a1.client_oid, a2.client_oid);
    assert_ne!(a2.client_oid, a3.client_oid);
    client.close();
}

#[tokio::test]
async fn error_frame_resolves_as_failure() {
    let (url, listener) = bind_local().await;

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let Some(Ok(Message::Text(text))) = ws.next().await else {
            return;
        };
        let v: Value = serde_json::from_str(text.as_str()).unwrap();
        let client_oid = v["data"]["clientOid"].as_str().unwrap();
        let err = serde_json::json!({
            "id":   "srv-1",
            "type": "error",
            "code": "400100",
            "data": {"clientOid": client_oid, "msg": "balance insufficient"},
        });
        ws.send(Message::Text(err.to_string().into()))
            .await
            .unwrap();
    });

    let client = WsOrderClient::connect(url).await.expect("connect");
    let ack = client
        .place_order("XBTUSDTM", Side::Buy, 1, 10, OrderType::Market, None)
        .await
        .expect("ack returned");
    assert!(!ack.success);
    assert_eq!(ack.error_code.as_deref(), Some("400100"));
    assert_eq!(ack.error_msg.as_deref(), Some("balance insufficient"));
    client.close();
}

#[tokio::test]
async fn request_times_out_when_no_ack() {
    let (url, listener) = bind_local().await;

    // Server accepts but never replies — the client should time out.
    let server_ready = Arc::new(Notify::new());
    let server_signal = server_ready.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        server_signal.notify_one();
        // Drain inbound silently.
        while let Some(frame) = ws.next().await {
            if matches!(frame, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    });

    let client = WsOrderClient::connect(url)
        .await
        .expect("connect")
        // Tight timeout so the test runs fast.
        .with_request_timeout(Duration::from_millis(500));
    server_ready.notified().await;

    let result = client
        .place_order("XBTUSDTM", Side::Buy, 1, 10, OrderType::Market, None)
        .await;
    match result {
        Err(ExchangeError::Order(msg)) => {
            assert!(msg.contains("timed out"), "unexpected msg: {msg}");
        }
        other => panic!("expected timeout Order error, got {other:?}"),
    }
    client.close();
}

#[tokio::test]
async fn close_drops_pending_requests() {
    let (url, listener) = bind_local().await;

    // Server accepts, reads one frame, then closes the connection without acking.
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let _ = ws.next().await;
        let _ = ws.close(None).await;
    });

    let client = WsOrderClient::connect(url)
        .await
        .expect("connect")
        .with_request_timeout(Duration::from_secs(5));

    let result = client
        .place_order("XBTUSDTM", Side::Buy, 1, 10, OrderType::Market, None)
        .await;
    match result {
        Ok(ack) => {
            // The reader task drained pending entries with a sentinel ack
            // before the future timed out — also acceptable behaviour.
            assert!(!ack.success);
            assert_eq!(ack.error_code.as_deref(), Some("connection_closed"));
        }
        Err(ExchangeError::Order(msg)) => {
            // Channel closed before ack arrived — the explicit-error path.
            assert!(
                msg.contains("closed") || msg.contains("timed out"),
                "unexpected msg: {msg}"
            );
        }
        other => panic!("expected close-related result, got {other:?}"),
    }
}
