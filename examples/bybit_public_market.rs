//! Fetch Bybit linear-perp BTCUSDT ticker + orderbook + funding history.
//!
//! Hits the live v5 API — no credentials required. Run with:
//!
//! ```text
//! cargo run --example bybit_public_market
//! ```

use exchange_apiws::{BybitCategory, BybitRestClient};

#[tokio::main(flavor = "current_thread")]
async fn main() -> exchange_apiws::Result<()> {
    let client = BybitRestClient::new()?;
    let category = BybitCategory::Linear;
    let symbol = "BTCUSDT";

    // Ticker — single instrument.
    let tickers = client.get_tickers(category, Some(symbol)).await?;
    let t = tickers.list.first().expect("ticker present");
    println!(
        "{symbol} {} — last={:.2} bid={:?} ask={:?} mark={:?} funding={:?}",
        category.as_str(),
        t.last_price,
        t.bid1_price,
        t.ask1_price,
        t.mark_price,
        t.funding_rate,
    );

    // Top of book — 5 levels.
    let book = client.get_orderbook(category, symbol, 5).await?;
    let bids = book.bids_f64();
    let asks = book.asks_f64();
    println!(
        "Orderbook update_id={}  top bid={:.2}x{:.4}  top ask={:.2}x{:.4}",
        book.update_id, bids[0][0], bids[0][1], asks[0][0], asks[0][1],
    );

    // Last three funding-rate settlements.
    let rates = client.get_funding_rate(category, symbol, 3).await?;
    println!("Recent funding rates ({}):", rates.list.len());
    for r in &rates.list {
        let ts = chrono::DateTime::from_timestamp_millis(r.funding_rate_timestamp).map_or_else(
            || r.funding_rate_timestamp.to_string(),
            |d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        );
        println!("  {ts}  {:+.6}", r.funding_rate);
    }

    Ok(())
}
