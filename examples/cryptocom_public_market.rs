//! Fetch Crypto.com BTC_USDT ticker + orderbook + the latest
//! `mark_price` valuation for BTCUSD-PERP.
//!
//! Hits the live API — no credentials required. Run with:
//!
//! ```text
//! cargo run --example cryptocom_public_market
//! ```

use exchange_apiws::CryptocomRestClient;

#[tokio::main(flavor = "current_thread")]
async fn main() -> exchange_apiws::Result<()> {
    let client = CryptocomRestClient::new()?;

    let tickers = client.get_ticker(Some("BTC_USDT")).await?;
    if let Some(t) = tickers.first() {
        println!(
            "{} — last={:?}  bid={:?}  ask={:?}  24h vol={:?}",
            t.instrument, t.last_price, t.best_bid, t.best_ask, t.volume_24h,
        );
    }

    let book = client.get_orderbook("BTC_USDT", 10).await?;
    let bids = book.bids_f64();
    let asks = book.asks_f64();
    if !bids.is_empty() && !asks.is_empty() {
        println!(
            "Orderbook seq={}  top bid={:.2}x{:.4}  top ask={:.2}x{:.4}",
            book.sequence, bids[0][0], bids[0][1], asks[0][0], asks[0][1],
        );
    }

    // get_valuations returns a time-series; the last entry is the latest.
    let marks = client.get_valuations("BTCUSD-PERP", "mark_price").await?;
    if let Some(latest) = marks.last() {
        let ts = chrono::DateTime::from_timestamp_millis(latest.timestamp)
            .map_or_else(|| latest.timestamp.to_string(), |d| d.to_rfc3339());
        println!("BTCUSD-PERP mark price = {} (at {ts})", latest.value);
    }

    Ok(())
}
