//! Example: stream public market data from KuCoin Futures.
//!
//! Reads credentials from environment variables:
//!   KC_KEY, KC_SECRET, KC_PASSPHRASE
//!
//! Run with:
//!   cargo run --example market_data

use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tracing::info;
use tracing_subscriber::EnvFilter;

use exchange_apiws::{
    Credentials, KuCoinClient, KucoinEnv,
    actors::DataMessage,
    ws::{KucoinConnector, WsRunnerConfig, run_feed},
};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let symbol = "XBTUSDTM";

    // ── REST: quick snapshot before streaming ─────────────────────────────────
    let creds  = Credentials::from_env()?;
    let client = KuCoinClient::new(creds, KucoinEnv::LiveFutures);

    let balance = client.get_balance("USDT").await?;
    info!(balance, "USDT available balance");

    let position = client.get_position(symbol).await?;
    info!(
        qty   = position.current_qty,
        price = ?position.avg_entry_price,
        pnl   = ?position.unrealised_pnl,
        "current position"
    );

    let funding = client.get_funding_rate(symbol).await?;
    info!(rate = funding.value, "current funding rate");

    let mark = client.get_mark_price(symbol).await?;
    info!(price = mark.value, "current mark price");

    // ── WebSocket: public feed ────────────────────────────────────────────────
    let token = client.get_ws_token_public().await?;
    let conn  = Arc::new(KucoinConnector::new(&token, KucoinEnv::LiveFutures)?);

    let mut subs = vec![];
    if let Some(s) = conn.trade_subscription(symbol)           { subs.push(s); }
    if let Some(s) = conn.ticker_subscription(symbol)          { subs.push(s); }
    if let Some(s) = conn.orderbook_depth_subscription(symbol, 5) { subs.push(s); }

    let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let config = WsRunnerConfig::from_ping_interval(conn.ping_interval_secs);

    info!("starting WS feed for {symbol}");
    tokio::spawn(run_feed(
        conn.ws_url().to_string(),
        subs,
        conn,
        tx,
        config,
        shutdown_rx,
    ));

    // ── Consume messages ──────────────────────────────────────────────────────
    let mut count = 0usize;
    while let Some(msg) = rx.recv().await {
        count += 1;
        match msg {
            DataMessage::Trade(t) => {
                info!(
                    count,
                    sym  = %t.symbol,
                    side = ?t.side,
                    price = t.price,
                    size  = t.amount,
                    "trade"
                );
            }
            DataMessage::Ticker(t) => {
                info!(
                    count,
                    sym  = %t.symbol,
                    bid  = t.best_bid,
                    ask  = t.best_ask,
                    "ticker"
                );
            }
            DataMessage::OrderBook(ob) => {
                let top_bid = ob.bids.first().copied().unwrap_or([0.0, 0.0]);
                let top_ask = ob.asks.first().copied().unwrap_or([0.0, 0.0]);
                info!(
                    count,
                    sym      = %ob.symbol,
                    snapshot = ob.is_snapshot,
                    bid      = top_bid[0],
                    bid_qty  = top_bid[1],
                    ask      = top_ask[0],
                    ask_qty  = top_ask[1],
                    "orderbook"
                );
            }
            // Private events — not subscribed in this example.
            DataMessage::OrderUpdate(_)
            | DataMessage::PositionChange(_)
            | DataMessage::BalanceUpdate(_) => {}
        }

        // Stop after 50 messages for demo purposes.
        if count >= 50 {
            info!("received {count} messages — shutting down");
            let _ = shutdown_tx.send(true);
            break;
        }
    }

    Ok(())
}
