#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "cryptocom")]

//! Crypto.com WS integration test via a local `tokio-tungstenite` server.
//!
//! Verifies the things that distinguish Crypto.com from the other
//! WS exchanges:
//! - **Server-initiated heartbeat** — server sends
//!   `{"id":<N>,"method":"public/heartbeat"}` and the client (via
//!   `response_for`) must reply with
//!   `{"id":<N>,"method":"public/respond-heartbeat"}` echoing the
//!   same `id`.
//! - Subscribe-after-connect with the
//!   `{"id":N,"method":"subscribe","params":{"channels":[…]}}` shape.
//! - All four public channels (trade, ticker, candlestick, book) flow
//!   through `run_feed` as the unified `DataMessage` variants.
//!
//! Run with:
//! ```text
//! cargo test --test cryptocom_ws_mock
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::cryptocom::ws::CryptocomConnector;
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

const fn fast_config() -> WsRunnerConfig {
    WsRunnerConfig {
        // Idle ticks just drive the (disabled) idle check; Crypto.com
        // doesn't need app pings.
        ping_interval_secs: 30,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 1,
        max_reconnect_attempts: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 0,
        on_event: None,
    }
}

// One linear test exercises subscribe, heartbeat round-trip, and
// full delivery — splitting per-variant would duplicate scaffolding.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn run_feed_subscribes_responds_to_heartbeat_and_delivers_all_variants() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    let captured_subscribes = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_heartbeat_responses = Arc::new(Mutex::new(Vec::<Value>::new()));

    let frames: Vec<String> = vec![
        // Server-initiated heartbeat — client MUST respond with the
        // same id. We send this FIRST so we can capture the response
        // before the rest of the test progresses.
        r#"{"id":4242,"method":"public/heartbeat"}"#.into(),
        // trade — batch of two trades.
        r#"{"id":-1,"method":"subscribe","code":0,"result":{
            "instrument_name":"BTC_USDT","channel":"trade","subscription":"trade.BTC_USDT",
            "data":[
                {"i":"BTC_USDT","s":"buy","p":"96000","q":"0.05","t":1700000000000,"d":"id-1"},
                {"i":"BTC_USDT","s":"sell","p":"96005","q":"0.10","t":1700000000500,"d":"id-2"}
            ]
        }}"#.into(),
        // ticker
        r#"{"id":-1,"method":"subscribe","code":0,"result":{
            "instrument_name":"BTC_USDT","channel":"ticker","subscription":"ticker.BTC_USDT",
            "data":[{"i":"BTC_USDT","a":"96000","b":"95999","k":"96001","t":1700000000000}]
        }}"#.into(),
        // candlestick
        r#"{"id":-1,"method":"subscribe","code":0,"result":{
            "instrument_name":"BTC_USDT","channel":"candlestick","subscription":"candlestick.1m.BTC_USDT",
            "data":[{"o":"96000","h":"96100","l":"95900","c":"96050","v":"10.5","t":1700000000000}]
        }}"#.into(),
        // book snapshot
        r#"{"id":-1,"method":"subscribe","code":0,"result":{
            "instrument_name":"BTC_USDT","channel":"book","subscription":"book.BTC_USDT.10","type":"snapshot",
            "data":[{
                "asks":[["96001","0.5","1"]],
                "bids":[["96000","1.5","2"]],
                "t":1700000000000,"s":1
            }]
        }}"#.into(),
    ];

    // trade (2) + ticker + candle + book = 5 DataMessages.
    let expected_msgs = 5;

    let server_subs = captured_subscribes.clone();
    let server_hb_responses = captured_heartbeat_responses.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        for f in frames {
            ws.send(Message::Text(f.into())).await.unwrap();
        }

        // Drain inbound; capture subscribe frames and any
        // public/respond-heartbeat the client sends.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let s = text.to_string();
                    if let Ok(v) = serde_json::from_str::<Value>(&s)
                        && v.get("method").and_then(Value::as_str)
                            == Some("public/respond-heartbeat")
                    {
                        server_hb_responses.lock().await.push(v);
                        continue;
                    }
                    if s.contains("\"method\":\"subscribe\"") {
                        server_subs.lock().await.push(s);
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let subs = vec![CryptocomConnector::subscribe_frame(
        1,
        &[
            CryptocomConnector::trade_channel("BTC_USDT"),
            CryptocomConnector::ticker_channel("BTC_USDT"),
            CryptocomConnector::candlestick_channel("BTC_USDT", "1m"),
            CryptocomConnector::book_channel("BTC_USDT", 10),
        ],
    )];
    let connector = Arc::new(CryptocomConnector::with_url(&url));

    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);
    let feed = tokio::spawn(run_feed(
        url,
        subs,
        connector.clone() as Arc<dyn ExchangeConnector>,
        tx,
        fast_config(),
        sd_rx,
    ));

    let mut got = Vec::with_capacity(expected_msgs);
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(m) => {
                    got.push(m);
                    if got.len() == expected_msgs { break; }
                }
                None => break,
            },
            () = &mut deadline => panic!(
                "timed out: expected {expected_msgs} messages, got {n}: {got:?}",
                n = got.len(),
            ),
        }
    }

    // Brief settle window for the heartbeat-response frame to reach
    // the server before we tear down.
    tokio::time::sleep(Duration::from_millis(300)).await;
    sd_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), feed).await;

    // 1) The runner emitted one subscribe frame with all 4 channels.
    let subs: Vec<String> = captured_subscribes.lock().await.clone();
    assert_eq!(subs.len(), 1);
    let sub_json: Value = serde_json::from_str(&subs[0]).unwrap();
    let channels = sub_json["params"]["channels"]
        .as_array()
        .expect("channels array");
    assert_eq!(channels.len(), 4);
    let channel_set: Vec<&str> = channels.iter().filter_map(Value::as_str).collect();
    assert!(channel_set.contains(&"trade.BTC_USDT"));
    assert!(channel_set.contains(&"ticker.BTC_USDT"));
    assert!(channel_set.contains(&"candlestick.1m.BTC_USDT"));
    assert!(channel_set.contains(&"book.BTC_USDT.10"));

    // 2) Server saw a heartbeat response — and it echoed the EXACT id
    //    we sent (4242).
    let hb: Vec<Value> = captured_heartbeat_responses.lock().await.clone();
    assert_eq!(hb.len(), 1, "expected exactly one heartbeat response");
    assert_eq!(hb[0]["id"], 4242, "heartbeat response must echo server id");
    assert_eq!(hb[0]["method"], "public/respond-heartbeat");

    // 3) All four DataMessage variants arrived end-to-end.
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
    let trade_count = kinds.iter().filter(|k| **k == "trade").count();
    assert_eq!(
        trade_count, 2,
        "expected 2 trades from the batch: {kinds:?}"
    );
    assert!(kinds.contains(&"ticker"));
    assert!(kinds.contains(&"candle"));
    assert!(kinds.contains(&"orderbook"));
}
