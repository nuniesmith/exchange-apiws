//! Fetch Kraken system status + XBT/USD ticker + recent OHLC.
//!
//! Hits the live API — no credentials required. Run with:
//!
//! ```text
//! cargo run --example kraken_public_market
//! ```

use exchange_apiws::KrakenRestClient;

#[tokio::main(flavor = "current_thread")]
async fn main() -> exchange_apiws::Result<()> {
    let client = KrakenRestClient::new()?;

    let status = client.get_system_status().await?;
    println!("System: {} (as of {})", status.status, status.timestamp);

    // Ticker — Kraken keys responses by its canonical pair name (e.g.
    // "XBTUSD" → "XXBTZUSD"), so iterate over the map.
    let tickers = client.get_ticker("XBTUSD").await?;
    for (pair, t) in &tickers {
        println!(
            "{pair} — last={:.2}  bid={:.2}  ask={:.2}  24h vol={:.4}",
            t.last_price(),
            t.bid_price(),
            t.ask_price(),
            t.volume_24h(),
        );
    }

    // 5 most recent 1-minute OHLC bars — `get_ohlc` returns raw
    // serde_json::Value because the response shape mixes per-pair
    // arrays with a top-level "last" cursor.
    let ohlc = client.get_ohlc("XBTUSD", 1).await?;
    if let Some(arr) = ohlc.get("XXBTZUSD").and_then(|v| v.as_array()) {
        println!("Last {} 1m OHLC bars:", arr.len().min(5));
        for bar in arr.iter().rev().take(5).rev() {
            // [time, open, high, low, close, vwap, volume, count]
            let close = bar.get(4).and_then(|v| v.as_str()).unwrap_or("?");
            let vol = bar.get(6).and_then(|v| v.as_str()).unwrap_or("?");
            let ts = bar.get(0).and_then(serde_json::Value::as_i64).unwrap_or(0);
            let ts_fmt = chrono::DateTime::from_timestamp(ts, 0)
                .map_or_else(|| ts.to_string(), |d| d.format("%H:%M:%S").to_string());
            println!("  {ts_fmt}  close={close}  vol={vol}");
        }
    }

    Ok(())
}
