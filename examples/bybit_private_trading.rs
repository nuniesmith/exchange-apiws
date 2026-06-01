//! Bybit **signed** REST — wallet balance, positions, open orders, and
//! (optionally) a live order round-trip.
//!
//! Requires `BYBIT_API_KEY` + `BYBIT_API_SECRET` in the environment. Defaults
//! to **testnet** so you can run it safely; set `BYBIT_TESTNET=false` to hit
//! mainnet. Reads are always performed; an order is only placed/cancelled when
//! `BYBIT_DEMO_ORDER=1` is set (and even then it's a far-from-market limit so
//! it rests and is immediately cancelled).
//!
//! ```text
//! BYBIT_API_KEY=… BYBIT_API_SECRET=… cargo run --example bybit_private_trading
//! BYBIT_API_KEY=… BYBIT_API_SECRET=… BYBIT_DEMO_ORDER=1 cargo run --example bybit_private_trading
//! ```

use exchange_apiws::{
    BybitCategory, BybitCredentials, BybitOrderRequest, BybitOrderSide, BybitPrivateClient,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> exchange_apiws::Result<()> {
    let creds = match BybitCredentials::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("set BYBIT_API_KEY and BYBIT_API_SECRET to run this example ({e})");
            return Ok(());
        }
    };
    let testnet = std::env::var("BYBIT_TESTNET").map_or(true, |v| v != "false");
    let client = BybitPrivateClient::new(creds, testnet)?;
    let category = BybitCategory::Linear;
    let symbol = "BTCUSDT";

    println!(
        "== Bybit {} ==",
        if testnet { "TESTNET" } else { "MAINNET" }
    );

    // ── Reads (always safe) ───────────────────────────────────────────────────
    let bal = client.get_wallet_balance("UNIFIED").await?;
    println!("wallet (UNIFIED): {}", trim(&bal));

    let positions = client.get_positions(category, Some(symbol)).await?;
    println!("positions {symbol}: {}", trim(&positions));

    let open = client.get_open_orders(category, Some(symbol)).await?;
    println!("open orders {symbol}: {}", trim(&open));

    // ── Optional live order round-trip ─────────────────────────────────────────
    if std::env::var("BYBIT_DEMO_ORDER").as_deref() == Ok("1") {
        // A tiny limit far below market → rests, doesn't fill. Adjust qty to
        // the symbol's min order size if the exchange rejects it.
        let order =
            BybitOrderRequest::limit(category, symbol, BybitOrderSide::Buy, "0.001", "10000")
                .with_order_link_id("exchange-apiws-demo")
                .reduce_only();

        match client.place_order(&order).await {
            Ok(ack) => {
                println!(
                    "placed order id={} link={}",
                    ack.order_id, ack.order_link_id
                );
                let cancelled = client.cancel_order(category, symbol, &ack.order_id).await?;
                println!("cancelled order id={}", cancelled.order_id);
            }
            Err(e) => println!("order rejected (expected on a fresh testnet acct): {e}"),
        }
    } else {
        println!("(set BYBIT_DEMO_ORDER=1 to place + cancel a resting demo limit)");
    }

    Ok(())
}

/// Compact a JSON value to a short one-line preview.
fn trim(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}
