#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "bybit")]

//! Bybit WS integration test via a local `tokio-tungstenite` server.
//!
//! Verifies the two paths that distinguish Bybit from Binance:
//! - The subscribe-after-connect flow: server captures the subscribe
//!   frame and asserts its shape (`{"op":"subscribe","args":[…]}`).
//! - The Bybit ping JSON format: server captures the ping frame and
//!   asserts it's `{"op":"ping"}`, not KuCoin's `{"type":"ping"}`.
//!
//! Both happen alongside frame delivery for trade / ticker / kline /
//! orderbook so the full Bybit pipeline is exercised end-to-end.
//!
//! Run with:
//! ```text
//! cargo test --test bybit_ws_mock
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::bybit::{BybitCategory, BybitConnector};
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

/// `ping_interval_secs = 1` so the server captures a ping inside the test
/// deadline; `idle_timeout_secs = 0` to disable the idle check (the server
/// doesn't reply to pings).
const fn fast_config() -> WsRunnerConfig {
    WsRunnerConfig {
        ping_interval_secs: 1,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 1,
        max_reconnect_attempts: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 0,
        on_event: None,
    }
}

// One linear test exercises subscribe, ping, and the full variant
// delivery — splitting it into smaller tests would require duplicating
// the server-handshake scaffolding for each path.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn run_feed_subscribes_pings_and_delivers_all_variants() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    // Shared state the server fills in so the test can assert later.
    let captured_subscribe = Arc::new(Mutex::new(None::<String>));
    let saw_ping = Arc::new(AtomicBool::new(false));

    let frames: Vec<String> = vec![
        // publicTrade — array of 2 trades
        r#"{"topic":"publicTrade.BTCUSDT","type":"snapshot","ts":1700000000000,"data":[
            {"T":1700000000050,"s":"BTCUSDT","S":"Buy","v":"0.1","p":"96000.0","L":"PlusTick","i":"id-1","BT":false},
            {"T":1700000000080,"s":"BTCUSDT","S":"Sell","v":"0.05","p":"96005.0","L":"MinusTick","i":"id-2","BT":false}
        ]}"#.into(),
        // tickers
        r#"{"topic":"tickers.BTCUSDT","type":"snapshot","ts":1700000000000,"data":{
            "symbol":"BTCUSDT","lastPrice":"96000.0","bid1Price":"95999.0","bid1Size":"1.0","ask1Price":"96001.0","ask1Size":"1.5"
        }}"#.into(),
        // kline (closed)
        r#"{"topic":"kline.1.BTCUSDT","type":"snapshot","ts":1700000000050,"data":[{
            "start":1700000000000,"end":1700000059999,"interval":"1","open":"96000.0","close":"96100.0","high":"96200.0","low":"95900.0","volume":"10.0","turnover":"961000.0","confirm":true,"timestamp":1700000000050
        }]}"#.into(),
        // orderbook snapshot
        r#"{"topic":"orderbook.50.BTCUSDT","type":"snapshot","ts":1700000000000,"data":{
            "s":"BTCUSDT","b":[["96000.0","1.5"]],"a":[["96001.0","0.5"]],"u":1,"seq":1
        }}"#.into(),
    ];

    let frame_count = frames.len();

    let server_subscribe = captured_subscribe.clone();
    let server_saw_ping = saw_ping.clone();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = accept_async(stream).await.expect("handshake");

        for f in frames {
            ws.send(Message::Text(f.into())).await.unwrap();
        }

        // Drain inbound frames; capture the first subscribe message and
        // flag any ping we observe.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let s = text.to_string();
                    if s.contains("\"op\":\"subscribe\"") {
                        let mut g = server_subscribe.lock().await;
                        if g.is_none() {
                            *g = Some(s);
                        }
                    } else if s == r#"{"op":"ping"}"# {
                        server_saw_ping.store(true, Ordering::SeqCst);
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let topics = vec![
        BybitConnector::trade_topic("BTCUSDT"),
        BybitConnector::ticker_topic("BTCUSDT"),
        BybitConnector::kline_topic("BTCUSDT", "1"),
        BybitConnector::orderbook_topic("BTCUSDT", 50),
    ];
    let connector = Arc::new(BybitConnector::with_url(
        &url,
        BybitCategory::Linear,
        topics,
    ));
    let sub = connector
        .subscription_message("")
        .expect("subscription message");

    let (tx, mut rx) = mpsc::channel::<DataMessage>(32);
    let feed = tokio::spawn(run_feed(
        url,
        vec![sub],
        connector.clone() as Arc<dyn ExchangeConnector>,
        tx,
        fast_config(),
        sd_rx,
    ));

    // The publicTrade frame yields two messages, so we expect 5 total
    // DataMessages from 4 source frames.
    let expected = frame_count + 1;
    let mut got = Vec::with_capacity(expected);
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(m) => {
                    got.push(m);
                    if got.len() >= expected { break; }
                }
                None => break,
            },
            () = &mut deadline => panic!(
                "timed out: expected {expected} messages, got {n}: {got:?}",
                n = got.len(),
            ),
        }
    }

    // Wait a moment for a ping tick to fire and reach the server.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    sd_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), feed).await;

    // 1) Subscribe frame is `{"op":"subscribe","args":[…]}` and contains
    //    all four topics we asked for.
    let sub_text = captured_subscribe
        .lock()
        .await
        .clone()
        .expect("server should have captured a subscribe frame");
    let sub_json: serde_json::Value = serde_json::from_str(&sub_text).expect("subscribe json");
    assert_eq!(sub_json["op"], "subscribe");
    let args = sub_json["args"]
        .as_array()
        .expect("subscribe args is an array");
    assert_eq!(args.len(), 4);
    assert!(args.iter().any(|v| v == "publicTrade.BTCUSDT"));
    assert!(args.iter().any(|v| v == "orderbook.50.BTCUSDT"));

    // 2) The runner sent at least one `{"op":"ping"}` — i.e. the Bybit
    //    ping format, not KuCoin's `{"type":"ping"}`.
    assert!(
        saw_ping.load(Ordering::SeqCst),
        "expected the runner to emit a Bybit-format ping",
    );

    // 3) Each DataMessage variant arrived through the pipeline.
    let kinds: Vec<&'static str> = got
        .iter()
        .map(|m| match m {
            DataMessage::Trade(_) => "trade",
            DataMessage::Ticker(_) => "ticker",
            DataMessage::Candle(_) => "candle",
            DataMessage::OrderBook(_) => "orderbook",
            _ => "other",
        })
        .collect();
    assert!(kinds.contains(&"trade"), "missing trade: {kinds:?}");
    assert!(kinds.contains(&"ticker"), "missing ticker: {kinds:?}");
    assert!(kinds.contains(&"candle"), "missing candle: {kinds:?}");
    assert!(kinds.contains(&"orderbook"), "missing orderbook: {kinds:?}");
    // publicTrade carried two trades — make sure we got both.
    let trade_count = kinds.iter().filter(|k| **k == "trade").count();
    assert_eq!(
        trade_count, 2,
        "expected 2 trades from the batch: {kinds:?}"
    );
}
