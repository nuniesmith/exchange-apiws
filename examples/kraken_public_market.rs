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

    // 5 most recent 1-minute OHLC bars. `get_ohlc` now returns a typed
    // `KrakenOhlc` — the echoed pair name, the candle `Vec`, and a `last`
    // cursor split out of Kraken's mixed pair-array + "last" shape.
    let ohlc = client.get_ohlc("XBTUSD", 1).await?;
    println!(
        "Last {} 1m OHLC bars for {} (next `since` = {}):",
        ohlc.candles.len().min(5),
        ohlc.pair,
        ohlc.last,
    );
    for bar in ohlc.candles.iter().rev().take(5).rev() {
        let ts_fmt = chrono::DateTime::from_timestamp(bar.time, 0).map_or_else(
            || bar.time.to_string(),
            |d| d.format("%H:%M:%S").to_string(),
        );
        println!(
            "  {ts_fmt}  close={:.2}  vol={:.4}",
            bar.close_f64(),
            bar.volume_f64(),
        );
    }

    Ok(())
}
