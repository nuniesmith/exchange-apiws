//! Drive Binance and Bybit BTCUSDT trade feeds into one downstream
//! channel and print whichever exchange ticks first.
//!
//! Demonstrates the unified [`DataMessage`] API: the same downstream
//! handler works regardless of which connector produced the message.
//! Exits after receiving 20 trades total.
//!
//! Hits live exchanges — no credentials required. Run with:
//!
//! ```text
//! cargo run --example multi_exchange_aggregator
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::binance::BinanceConnector;
use exchange_apiws::bybit::{BybitCategory, BybitConnector};
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let (tx, mut rx) = mpsc::channel::<DataMessage>(1024);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Binance Spot — URL-encoded aggTrade stream, no subscribe frame.
    let binance_streams = [BinanceConnector::trade_stream("BTCUSDT")];
    let stream_refs: Vec<&str> = binance_streams.iter().map(String::as_str).collect();
    let binance = Arc::new(BinanceConnector::spot(&stream_refs));
    let binance_url = binance.ws_url().to_string();

    // Bybit Linear — JSON subscribe frame after connect.
    let bybit = Arc::new(BybitConnector::new(
        BybitCategory::Linear,
        vec![BybitConnector::trade_topic("BTCUSDT")],
    ));
    let bybit_url = bybit.ws_url().to_string();
    let bybit_subs: Vec<String> = bybit.subscription_message("").into_iter().collect();

    // Spawn both feeds.
    let binance_handle = tokio::spawn(run_feed(
        binance_url,
        vec![], // streams are encoded in the URL
        binance.clone() as Arc<dyn ExchangeConnector>,
        tx.clone(),
        WsRunnerConfig::default(),
        shutdown_rx.clone(),
    ));
    let bybit_handle = tokio::spawn(run_feed(
        bybit_url,
        bybit_subs,
        bybit.clone() as Arc<dyn ExchangeConnector>,
        tx,
        WsRunnerConfig::default(),
        shutdown_rx,
    ));

    let mut count = 0_usize;
    let deadline = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(DataMessage::Trade(t)) => {
                    count += 1;
                    println!(
                        "[{count:3}] {:<16} {} {:?} {:>10} @ {:>10}",
                        t.exchange, t.symbol, t.side, t.amount, t.price,
                    );
                    if count >= 20 { break; }
                }
                Some(_) => {} // non-trade variants ignored here
                None => break,
            },
            () = &mut deadline => {
                eprintln!("60 s deadline reached, exiting");
                break;
            }
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = binance_handle.await;
        let _ = bybit_handle.await;
    })
    .await;
    Ok(())
}
