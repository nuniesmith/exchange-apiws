# exchange-apiws

[![Crates.io](https://img.shields.io/crates/v/exchange-apiws.svg)](https://crates.io/crates/exchange-apiws)
[![Docs.rs](https://docs.rs/exchange-apiws/badge.svg)](https://docs.rs/exchange-apiws)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Async Rust client for exchange REST APIs and WebSocket feeds.

Six exchanges supported, three signing schemes, four envelope variants. The
crate is architected to be exchange-agnostic: adding a new exchange means
implementing one trait and the shared runner handles connection lifecycle,
reconnect, rate limiting, heartbeats, and supervised token refresh.

| Exchange | Public REST | Private REST | Public WS | Private WS | Notes |
|---|---|---|---|---|---|
| **KuCoin** (Futures + Spot + UTA) | ✓ | ✓ | ✓ | ✓ | + `WsOrderClient` for low-latency order placement |
| **Binance** | ✓ | — | ✓ | — | spot + USDT-M futures |
| **Bybit** | ✓ | — | ✓ | — | v5 unified API (spot / linear / inverse) |
| **Kraken** | ✓ | ✓ | ✓ | — | HMAC-SHA512 signing |
| **Crypto.com** | ✓ | ✓ | ✓ | — | HMAC-SHA256 body-`sig` signing |

The disconnection-hardening track lands `run_feed_supervised` (token-refresh
on cascade), connect / idle timeouts, cascade-start WARN logging, and a
`RunnerEvent` observability hook for metrics. See [the supervised-feed
example below](#supervised-websocket-feed-token-re-negotiation-on-cascade).

---

## Table of Contents

- [Status](#status)
- [Installation](#installation)
- [Quick Start](#quick-start)
  - [Public market data — Binance](#public-market-data--binance)
  - [Public market data — Bybit](#public-market-data--bybit)
  - [KuCoin futures REST](#kucoin-futures-rest)
  - [Public WebSocket feed (KuCoin)](#public-websocket-feed)
  - [Private WebSocket feed (order fills + positions)](#private-websocket-feed-order-fills--positions)
  - [Supervised WebSocket feed (token re-negotiation on cascade)](#supervised-websocket-feed-token-re-negotiation-on-cascade)
  - [Multi-exchange WS via the unified `DataMessage` types](#multi-exchange-ws-via-the-unified-datamessage-types)
- [Placing Orders](#placing-orders)
- [Rate Limits](#rate-limits)
- [Error Handling](#error-handling)
- [Authentication](#authentication)
- [KuCoin-Specific Notes](#kucoin-specific-notes)
- [Adding a new exchange](#adding-a-new-exchange)
- [License](#license)

---

## Status

Everything in the original roadmap is implemented. 305 tests across the
crate cover signing, envelope unwrapping, WS connector parsers, and the
runner lifecycle (reconnect, supervised token refresh, idle timeout,
event observability).

### Exchange-by-exchange surface

#### KuCoin

| Layer | What's covered |
|---|---|
| REST (Futures) | balance, positions, orders, stop orders, fills, klines, ticker, mark price, funding rate/history, contracts, transfers |
| REST (UTA) | account summary, cross/isolated margin accounts |
| REST (Spot margin) | place / get / cancel margin order, fills, account |
| WS (Public) | trade, ticker, orderbook (depth + L2 delta) |
| WS (Private) | order fills, position changes, balance updates, advanced (stop) orders |
| WS (Order placement) | `WsOrderClient` — low-latency place/cancel with `clientOid` routing |

#### Binance (public-only, no API keys)

| Layer | What's covered |
|---|---|
| REST (Spot) | klines, depth, recent trades, book ticker, 24h ticker, exchange info |
| REST (Futures USDT-M) | klines, funding rate history, premium index / mark price, open interest |
| WS | `<sym>@aggTrade`, `@bookTicker`, `@kline_<i>`, `@depth@100ms`, `@depth{5\|10\|20}@100ms`, `@markPrice@1s` |

#### Bybit (public-only, v5 API)

| Layer | What's covered |
|---|---|
| REST | kline, orderbook, tickers, recent trades, instruments info, funding history, open interest, long/short ratio |
| WS | `publicTrade.<sym>`, `tickers.<sym>`, `kline.<i>.<sym>`, `orderbook.<d>.<sym>` |

#### Kraken (signed)

| Layer | What's covered |
|---|---|
| REST (Public) | system status, assets, asset pairs, ticker, depth, OHLC, recent trades, spread |
| REST (Private, HMAC-SHA512) | balance, open/closed orders, place/cancel/cancel-all order, trades history, ledger, withdraw, withdrawal status |
| WS (Public v2) | `trade`, `ticker`, `ohlc`, `book` |

#### Crypto.com (signed)

| Layer | What's covered |
|---|---|
| REST (Public) | instruments, book, candlestick, ticker, trades, valuations (mark/funding/index) |
| REST (Private, HMAC-SHA256 body-`sig`) | account summary, create/cancel order, cancel-all, open orders, order detail, trades, deposit address, create/list withdrawal |
| WS (Public) | `trade.<inst>`, `ticker.<inst>`, `candlestick.<tf>.<inst>`, `book.<inst>.<d>` |

### Disconnection hardening

The shared `run_feed` runner ships with:

- **`run_feed_supervised`** — wraps `run_feed` in a token-refresh loop;
  on cascade exhaustion it calls a caller-supplied closure for a fresh
  endpoint instead of returning `WsDisconnected`. Drops typical
  stale-token blackout from ~9 min to ~10 s.
- **`connect_timeout_secs`** + **`idle_timeout_secs`** — bound stalled
  handshakes and half-closed TCP.
- **Cascade-start WARN** — first session-end of a new reconnect chain
  (attempt 0, sub-5 s uptime) logs at WARN with the close-frame reason,
  so production logs filtered at WARN show the root cause.
- **`RunnerEvent` observability hook** — `SessionEnded`,
  `ReconnectsExhausted`, `TokenRefresh`, `RefreshExhausted` callbacks
  for metrics without log scraping.

Default `WsRunnerConfig` is tuned for the futures-bot use case: 5
attempts × 30 s ceiling ≈ 95 s worst-case before the supervisor steps
in.

---

## Installation

```toml
[dependencies]
exchange-apiws = "0.2"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

### Per-exchange Cargo features

The four non-KuCoin exchanges are opt-out via Cargo features
(`binance`, `bybit`, `kraken`, `cryptocom` — all in `default`). Trim
the dependency footprint by disabling unused exchanges:

```toml
# KuCoin-only
exchange-apiws = { version = "0.2", default-features = false }

# KuCoin + Binance
exchange-apiws = { version = "0.2", default-features = false, features = ["binance"] }
```

KuCoin and the shared runtime (`actors`, `client`, `auth`, `http`,
`rest`, `ws`) stay always-on — they're the runtime infrastructure
the other exchanges build on.

Set credentials per the exchange you're using:

```
# KuCoin
KC_KEY=...
KC_SECRET=...
KC_PASSPHRASE=...

# Kraken (private REST)
KRAKEN_API_KEY=...
KRAKEN_API_SECRET=...

# Crypto.com (private REST)
CRYPTOCOM_API_KEY=...
CRYPTOCOM_API_SECRET=...
```

Public-only exchanges (Binance, Bybit) need no credentials.

### Runnable examples

The `examples/` directory has a runnable binary per exchange plus
two cross-cutting demos. Run any one with `cargo run --example <name>`:

| Example | What it does |
|---|---|
| `binance_public_market` | Spot klines + 24h ticker + futures mark price for BTCUSDT |
| `bybit_public_market` | Linear-perp ticker + 5-level orderbook + recent funding history |
| `kraken_public_market` | System status + XBT/USD ticker + recent 1m OHLC bars |
| `cryptocom_public_market` | BTC_USDT ticker + 10-level orderbook + BTCUSD-PERP mark price |
| `multi_exchange_aggregator` | Drives Binance + Bybit BTCUSDT trade feeds into one channel; demonstrates the unified `DataMessage` cross-exchange pattern |
| `kucoin_supervised_feed` | **Recommended production pattern** — `run_feed_supervised` for token re-negotiation on cascade, with a `RunnerEvent` listener wired to a metrics counter and Ctrl-C shutdown |

---

## Quick Start

> **Tip:** `use exchange_apiws::prelude::*;` brings the error types, the
> unified `DataMessage` model + `ExchangeConnector` trait, the WS runner
> entry points (`run_feed`, `run_feed_supervised`), and every enabled
> exchange's client + connector into scope in one line. The examples
> below import explicit paths for clarity.

### Public market data — Binance

```rust
use exchange_apiws::BinanceRestClient;

# async fn ex() -> exchange_apiws::Result<()> {
let client = BinanceRestClient::new()?;
let klines = client.get_klines("BTCUSDT", "1m", 100).await?;
let ticker = client.get_ticker("BTCUSDT").await?;
let funding = client.get_futures_funding_rate("BTCUSDT", 5).await?;
println!("latest close: {}  bid: {}  funding: {:?}",
    klines.last().unwrap().close, ticker.bid_price, funding);
# Ok(())
# }
```

### Public market data — Bybit

```rust
use exchange_apiws::{BybitCategory, BybitRestClient};

# async fn ex() -> exchange_apiws::Result<()> {
let client = BybitRestClient::new()?;
let bars = client.get_klines(BybitCategory::Linear, "BTCUSDT", "1", 100).await?;
let book = client.get_orderbook(BybitCategory::Linear, "BTCUSDT", 50).await?;
println!("{} bars, top bid {}", bars.list.len(), book.bids_f64()[0][0]);
# Ok(())
# }
```

### KuCoin futures REST

```rust
use exchange_apiws::{Credentials, KuCoin};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let client = KuCoin::futures(Credentials::from_env()?).rest_client()?;

    let balance  = client.get_balance("USDT").await?;
    let position = client.get_position("XBTUSDTM").await?;
    let candles  = client.fetch_klines("XBTUSDTM", 200, "1").await?;
    let funding  = client.get_funding_rate("XBTUSDTM").await?;

    println!("balance={balance:.2}  qty={}  funding={:.6}", position.current_qty, funding.value);
    Ok(())
}
```

### Public WebSocket feed

KuCoin Futures public feed:

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use exchange_apiws::{Credentials, KuCoin, actors::DataMessage};
use exchange_apiws::ws::{KucoinConnector, WsRunnerConfig, run_feed};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let kucoin = KuCoin::futures(Credentials::from_env()?);
    let client = kucoin.rest_client()?;
    let token  = client.get_ws_token_public().await?;
    let conn   = Arc::new(KucoinConnector::new(&token, kucoin.env())?);

    let subs = vec![
        conn.trade_subscription("XBTUSDTM").unwrap(),
        conn.ticker_subscription("XBTUSDTM").unwrap(),
    ];

    let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(run_feed(
        conn.ws_url().to_string(),
        subs,
        conn,
        tx,
        WsRunnerConfig::default(),
        shutdown_rx,
    ));

    while let Some(msg) = rx.recv().await {
        println!("{msg:?}");
    }
    let _ = shutdown_tx.send(true);
    Ok(())
}
```

### Private WebSocket feed (order fills + positions)

```rust
// Use get_ws_token_private() and add private subscriptions:
let kucoin = KuCoin::futures(Credentials::from_env()?);
let client = kucoin.rest_client()?;
let token  = client.get_ws_token_private().await?;
let conn   = Arc::new(KucoinConnector::new(&token, kucoin.env())?);

let subs = vec![
    conn.order_updates_subscription().unwrap(),   // fills & status changes
    conn.position_subscription("XBTUSDTM").unwrap(),
    conn.balance_subscription().unwrap(),
];
// ... same run_feed setup as above
```

### Supervised WebSocket feed (token re-negotiation on cascade)

`run_feed` retries inside one token. If the disconnect cause is a stale or
invalidated token (KuCoin's gateway closing freshly subscribed sessions, for
example), retrying the dead endpoint can burn the full reconnect budget —
up to **~9 minutes of blackout with default settings**. `run_feed_supervised`
wraps `run_feed` in an outer loop that calls a caller-supplied closure to
re-negotiate a fresh token whenever a cycle exhausts, typically restoring
the feed in seconds.

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use exchange_apiws::{Credentials, KuCoin, actors::DataMessage};
use exchange_apiws::ws::{
    KucoinConnector, SupervisedConfig, WsFeedEndpoint, run_feed_supervised,
};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let kucoin = KuCoin::futures(Credentials::from_env()?);
    let client = Arc::new(kucoin.rest_client()?);
    let env    = kucoin.env();

    // Parsing is URL-independent — one connector handles all cycles.
    let initial = client.get_ws_token_public().await?;
    let connector = Arc::new(KucoinConnector::new(&initial, env)?);

    // Called on bootstrap and after every cascade.
    let refresh = {
        let client = client.clone();
        move || {
            let client = client.clone();
            async move {
                let token = client.get_ws_token_public().await?;
                let conn  = KucoinConnector::new(&token, env)?;
                let subs  = vec![
                    conn.trade_subscription("XBTUSDTM").unwrap(),
                    conn.ticker_subscription("XBTUSDTM").unwrap(),
                ];
                Ok(WsFeedEndpoint {
                    url: conn.ws_url().to_string(),
                    subscriptions: subs,
                })
            }
        }
    };

    let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(run_feed_supervised(
        connector,
        tx,
        SupervisedConfig::default(),  // per-cycle budget of 3, unlimited cycles
        shutdown_rx,
        refresh,
    ));

    while let Some(msg) = rx.recv().await {
        println!("{msg:?}");
    }
    let _ = shutdown_tx.send(true);
    Ok(())
}
```

`SupervisedConfig::default()` sets `runner.max_reconnect_attempts = 3` so
cascades are detected in ~35 s rather than ~9 min, and
`max_refresh_cycles = u32::MAX` so the supervisor keeps refreshing until you
trigger `shutdown_tx.send(true)`. For a bounded version that surfaces
`WsDisconnected` after N refresh cycles, override `max_refresh_cycles`.

### Contract sizing

`calc_contracts` is an async method on `KuCoinClient` — it calls `GET /api/v1/contracts/{symbol}` to retrieve the contract multiplier at runtime, so it returns a `Result`.

```rust
let client    = KuCoin::futures(Credentials::from_env()?).rest_client()?;
let contracts = client.calc_contracts(
    "XBTUSDTM",
    96_000.0,   // current price
    1_000.0,    // available balance (USDT)
    10,         // leverage
    0.02,       // risk 2% of balance
    50,         // max contracts cap
).await?;
println!("{contracts} contracts");
```

### Multi-exchange WS via the unified `DataMessage` types

Every connector implements `ExchangeConnector` and emits the same
`DataMessage` enum (`Trade`, `Ticker`, `Candle`, `OrderBook`,
`FundingRate`, …). The same downstream handler works for KuCoin,
Binance, Bybit, Kraken, and Crypto.com feeds.

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::binance::BinanceConnector;
use exchange_apiws::bybit::{BybitCategory, BybitConnector};
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

# async fn ex() -> exchange_apiws::Result<()> {
let (tx, mut rx) = mpsc::channel::<DataMessage>(2048);
let (_sd_tx, sd_rx) = watch::channel(false);

// Two feeds, two connections, ONE downstream channel.
let binance = Arc::new(BinanceConnector::spot(&[
    &BinanceConnector::trade_stream("BTCUSDT"),
    &BinanceConnector::kline_stream("BTCUSDT", "1m"),
]));
let bybit = Arc::new(BybitConnector::new(
    BybitCategory::Linear,
    vec![
        BybitConnector::trade_topic("BTCUSDT"),
        BybitConnector::kline_topic("BTCUSDT", "1"),
    ],
));
let bybit_subs = bybit.subscription_message("").into_iter().collect();

let (binance_url, bybit_url) = (binance.ws_url().to_string(), bybit.ws_url().to_string());
tokio::spawn(run_feed(binance_url, vec![], binance, tx.clone(), WsRunnerConfig::default(), sd_rx.clone()));
tokio::spawn(run_feed(bybit_url, bybit_subs, bybit, tx, WsRunnerConfig::default(), sd_rx));

while let Some(msg) = rx.recv().await {
    match msg {
        DataMessage::Trade(t) => println!("[{}] {} {:?} {}@{}",
            t.exchange, t.symbol, t.side, t.amount, t.price),
        DataMessage::Candle(c) => println!("[{}] {} {} OHLC {}-{}-{}-{}",
            c.exchange, c.symbol, c.interval, c.open, c.high, c.low, c.close),
        _ => {}
    }
}
# Ok(())
# }
```

---

## Placing Orders

```rust
use exchange_apiws::types::{Side, OrderType, TimeInForce, STP};

// Market order
client.place_order(
    "XBTUSDTM", Side::Buy, 5, 10,
    OrderType::Market, None, false, None,
).await?;

// Limit order with IOC + STP
client.place_order(
    "XBTUSDTM", Side::Sell, 3, 10,
    OrderType::Limit,
    Some(TimeInForce::IOC),
    false,
    Some(STP::CN),
).await?;

// Stop-market order (close on breach)
client.place_stop_order(
    "XBTUSDTM", Side::Sell, 5, 10,
    93_000.0, "down", None, true,
).await?;
```

---

## Rate Limits

### REST

KuCoin enforces per-UID rate limits per resource pool. VIP0 Futures quota is 2,000 requests / 30 seconds. The client automatically:

- Retries transient failures with exponential backoff (3 attempts, 1.5× factor)
- Reads the `gw-ratelimit-reset` header on HTTP 429 and sleeps for the exact reset window
- Returns `ExchangeError::Api` with the KuCoin error code on non-200000 responses

### WebSocket

KuCoin allows **100 client→server messages per 10 seconds per connection** (subscribe, unsubscribe, ping). The runner enforces this with a sliding-window guard before every outbound send — subscriptions sent at startup are rate-limited too, so large subscription batches at connect time will be transparently throttled.

---

## Error Handling

All fallible functions return `Result<T>` where the error type is `ExchangeError`:

```rust
use exchange_apiws::ExchangeError;

match client.get_position("XBTUSDTM").await {
    Ok(pos)  => println!("{}", pos.current_qty),
    Err(ExchangeError::Api { code, message }) => eprintln!("KuCoin error {code}: {message}"),
    Err(ExchangeError::WsDisconnected)        => eprintln!("WS gave up after max reconnects"),
    Err(e)                                    => eprintln!("other error: {e}"),
}
```

---

## Authentication

KuCoin API v2 HMAC-SHA256 signing is implemented in `auth::build_headers`. The prehash string is `{timestamp}{METHOD}{endpoint}{body}`. The passphrase is itself HMAC-signed (not sent raw), which is the v2 requirement.

Credentials are loaded from environment variables with `Credentials::from_env()`:

| Variable | Description |
|----------|-------------|
| `KC_KEY` | API key |
| `KC_SECRET` | API secret |
| `KC_PASSPHRASE` | API passphrase |

---

## KuCoin-Specific Notes

**Leverage** is a per-order field in KuCoin Futures, not an account setting. Pass `leverage` in `place_order` and `close_position`. Use `set_risk_limit_level` to change the max position size tier.

**Inverse vs. linear contracts** — `calc_contracts` fetches the contract multiplier live via `get_contract`. Inverse (USD-margined) contracts like `XBTUSDM` have a multiplier of 1 USD. Linear (USDT-margined) contracts like `XBTUSDTM` express a base-coin multiplier (0.001 BTC per contract).

**Private WS token expiry** — WS tokens are valid for the lifetime of the connection. The runner reconnects automatically; call `get_ws_token_private()` again inside the reconnect flow if you need long-lived private feeds.

---

## Adding a new exchange

The plumbing is reusable. A new exchange typically needs three pieces:

1. **REST client** wrapping `PublicRestClient` (and adding a signing
   layer for authenticated calls). See `src/binance/rest.rs` for an
   unauthenticated example or `src/kraken/private.rs` for a signed one.
2. **Envelope unwrap** — a free function per exchange that strips the
   `{"code":N,"result":...}` (or equivalent) wrapper. Pattern is
   `unwrap_<exchange>_envelope<T>(raw: Value) -> Result<T>`; surface
   non-zero codes as `ExchangeError::Api`.
3. **`ExchangeConnector` implementation** for WebSocket. The trait
   provides three optional hooks with default impls so most connectors
   only override what they need:
   - `subscription_message(symbol)` — for one-frame-per-subscribe
     protocols
   - `ping_message()` — application-level ping format (`None` if the
     exchange uses protocol-level Ping or server-initiated heartbeats)
   - `response_for(raw)` — inbound-driven outbound (Crypto.com's
     heartbeat-echo pattern is the canonical user)

Once the connector is implemented, the shared runner (`run_feed`,
`run_feed_supervised`) handles reconnect, idle timeout, rate
limiting, observability events, and supervised token refresh.

`KrakenConnector` (subscribe-after-connect, multi-subscribe) and
`CryptocomConnector` (server-initiated heartbeat) are the two
worked examples of how the trait extensions cover non-trivial
exchange protocols.

---

## Changelog

Version history with grouped Added / Changed / Fixed entries:
[CHANGELOG.md](CHANGELOG.md).

## License

MIT — see [LICENSE](LICENSE).
