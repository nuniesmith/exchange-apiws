#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "kraken")]

//! Kraken v2 WS integration test via a local `tokio-tungstenite` server.
//!
//! Verifies the three things that distinguish Kraken from Bybit / Binance:
//! - The subscribe-after-connect flow with `{"method":"subscribe",…}`
//!   frames — one per channel (not bundled, unlike Bybit's batched
//!   `{"op":"subscribe","args":[…]}`).
//! - The Kraken-specific ping JSON format (`{"method":"ping"}`).
//! - The four public channels (trade, ticker, ohlc, book) all flow
//!   through `run_feed` as the unified `DataMessage` variants.
//!
//! Run with:
//! ```text
//! cargo test --test kraken_ws_mock
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::kraken::ws::KrakenConnector;
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

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

// One linear test exercises subscribe, ping, and full delivery —
// splitting per-variant would duplicate the server-handshake scaffold.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn run_feed_subscribes_pings_and_delivers_all_variants() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    let captured_subscribes = Arc::new(Mutex::new(Vec::<String>::new()));
    let saw_ping = Arc::new(AtomicBool::new(false));

    let frames: Vec<String> = vec![
        // trade — array of two trades to confirm batch handling
        r#"{"channel":"trade","type":"snapshot","data":[
            {"symbol":"BTC/USD","side":"buy","qty":0.1,"price":96000.0,"ord_type":"market","trade_id":1,"timestamp":"2026-05-25T12:00:00.000000Z"},
            {"symbol":"BTC/USD","side":"sell","qty":0.05,"price":96005.0,"ord_type":"limit","trade_id":2,"timestamp":"2026-05-25T12:00:00.500000Z"}
        ]}"#.into(),
        // ticker
        r#"{"channel":"ticker","type":"snapshot","data":[
            {"symbol":"BTC/USD","bid":95999.0,"ask":96001.0,"bid_qty":1.0,"ask_qty":1.5,"last":96000.0,"volume":100.5,"high":96500.0,"low":95500.0,"vwap":95800.0,"change":250.0,"change_pct":0.26}
        ]}"#.into(),
        // ohlc
        r#"{"channel":"ohlc","type":"snapshot","data":[
            {"symbol":"BTC/USD","interval":1,"open":96000.0,"high":96100.0,"low":95900.0,"close":96050.0,"trades":100,"volume":10.5,"vwap":96025.0,"interval_begin":"2026-05-25T12:00:00.000000Z"}
        ]}"#.into(),
        // book snapshot
        r#"{"channel":"book","type":"snapshot","data":[
            {"symbol":"BTC/USD","bids":[{"price":96000.0,"qty":1.5}],"asks":[{"price":96001.0,"qty":0.5}],"checksum":1}
        ]}"#.into(),
        // heartbeat should be ignored — included to verify it doesn't
        // produce a DataMessage or trip the parser.
        r#"{"channel":"heartbeat"}"#.into(),
    ];

    // 1 (trade snapshot, 2 trades) + 1 ticker + 1 candle + 1 book = 5 DataMessages.
    let expected_msgs = 5;

    let server_subs = captured_subscribes.clone();
    let server_saw_ping = saw_ping.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        for f in frames {
            ws.send(Message::Text(f.into())).await.unwrap();
        }

        // Drain inbound; capture subscribe frames and flag any ping.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let s = text.to_string();
                    if s.contains("\"method\":\"subscribe\"") {
                        server_subs.lock().await.push(s);
                    } else if s == r#"{"method":"ping"}"# {
                        server_saw_ping.store(true, Ordering::SeqCst);
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let subs = vec![
        KrakenConnector::trade_subscription(&["BTC/USD"]),
        KrakenConnector::ticker_subscription(&["BTC/USD"]),
        KrakenConnector::ohlc_subscription(&["BTC/USD"], 1),
        KrakenConnector::book_subscription(&["BTC/USD"], 10),
    ];
    let connector = Arc::new(KrakenConnector::with_url(&url));

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

    // Wait briefly for a ping tick to reach the server.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    sd_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), feed).await;

    // 1) The runner sent ONE subscribe frame per channel (4 total).
    // Snapshot the captured subscribes and release the lock immediately.
    let subs: Vec<String> = captured_subscribes.lock().await.clone();
    assert_eq!(
        subs.len(),
        4,
        "expected 4 subscribe frames, got {}",
        subs.len()
    );
    let channels: Vec<String> = subs
        .iter()
        .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .filter_map(|v| v["params"]["channel"].as_str().map(str::to_string))
        .collect();
    assert!(channels.contains(&"trade".to_string()));
    assert!(channels.contains(&"ticker".to_string()));
    assert!(channels.contains(&"ohlc".to_string()));
    assert!(channels.contains(&"book".to_string()));

    // 2) Server saw the Kraken-format ping (NOT KuCoin's, NOT Bybit's).
    assert!(
        saw_ping.load(Ordering::SeqCst),
        "expected runner to emit Kraken-format ping {{\"method\":\"ping\"}}",
    );

    // 3) All four DataMessage variants arrived; trades batch produced 2.
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
    assert_eq!(trade_count, 2, "expected 2 trades from batch: {kinds:?}");
    assert!(kinds.contains(&"ticker"), "missing ticker: {kinds:?}");
    assert!(kinds.contains(&"candle"), "missing candle: {kinds:?}");
    assert!(kinds.contains(&"orderbook"), "missing orderbook: {kinds:?}");
}
