//! Fetch Binance Spot klines + the 24h ticker for BTCUSDT, and pull
//! the latest futures mark-price snapshot.
//!
//! Hits the live API — no credentials required. Run with:
//!
//! ```text
//! cargo run --example binance_public_market
//! ```

use exchange_apiws::BinanceRestClient;

#[tokio::main(flavor = "current_thread")]
async fn main() -> exchange_apiws::Result<()> {
    let client = BinanceRestClient::new()?;

    // 50 most recent 1-minute spot bars for BTCUSDT.
    let bars = client.get_klines("BTCUSDT", "1m", 50).await?;
    let last = bars.last().expect("at least one bar");
    println!(
        "Spot BTCUSDT — last 1m bar: O={:.2} H={:.2} L={:.2} C={:.2}  vol={:.4}",
        last.open, last.high, last.low, last.close, last.volume,
    );

    // 24-hour rolling ticker.
    let t24 = client.get_ticker_24h("BTCUSDT").await?;
    println!(
        "Spot BTCUSDT — 24h: last={:.2}  Δ={:+.2}%  vol={:.2} BTC",
        t24.last_price, t24.price_change_percent, t24.volume,
    );

    // USDT-M futures: mark price + next-funding info.
    let mp = client.get_futures_mark_price("BTCUSDT").await?;
    println!(
        "Futures BTCUSDT — mark={:.2}  index={:.2}  last_funding={:+.6}  next funding at {}",
        mp.mark_price,
        mp.index_price,
        mp.last_funding_rate,
        chrono::DateTime::from_timestamp_millis(mp.next_funding_time)
            .map_or_else(|| mp.next_funding_time.to_string(), |d| d.to_rfc3339()),
    );

    Ok(())
}
