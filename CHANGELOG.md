# Changelog

All notable changes to **`exchange-apiws`** are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

— nothing yet —

## [0.2.19] – 2026-05-27

### Added

- **Per-exchange Cargo features** — `binance`, `bybit`, `kraken`,
  `cryptocom`. All four are in the `default` feature set so existing
  users see no change. Downstream crates can opt out via
  `default-features = false`:
  ```toml
  exchange-apiws = { version = "0.2", default-features = false, features = ["binance"] }
  ```
  KuCoin (and the shared runtime: `actors`, `client`, `auth`,
  `connectors`, `http`, `rest`, `ws`, `types`) stay always-on — most
  of that code is the runtime infrastructure the other exchanges
  build on. Integration tests and the `examples/` binaries gate
  themselves to their owning feature via `#![cfg(feature = "…")]`
  (tests) and `required-features` (`[[example]]` entries in
  `Cargo.toml`).

## [0.2.18] – 2026-05-27

### Added

- **`examples/` directory** — six runnable binaries (one per exchange
  plus `multi_exchange_aggregator` and `kucoin_supervised_feed`).
  `cargo run --example <name>` works for each. The README gains a
  table linking them all.
- `tokio` gains the `signal` feature in `[dev-dependencies]` for the
  Ctrl-C handler in `kucoin_supervised_feed`. Library `[dependencies]`
  is unchanged.

## [0.2.17] – 2026-05-27

### Added

- **`CHANGELOG.md`** in Keep a Changelog format, tracking every
  version 0.1.11 → 0.2.16 with grouped Added / Changed / Fixed
  entries and GitHub compare links. README links to it.

## [0.2.16] – 2026-05-27

### Changed

- Rewrote the README for the current six-exchange surface. Added a
  status matrix, per-exchange endpoint coverage tables, Quick Start
  sections for Binance and Bybit alongside KuCoin, and a
  multi-exchange `DataMessage` example. Dropped the 366-line stale
  Roadmap section (everything in it is implemented) and replaced it
  with a compact "Adding a new exchange" guide.

## [0.2.15] – 2026-05-27

### Added

- **Crypto.com WebSocket connector** — `CryptocomConnector` covers the
  four public channels (`trade`, `ticker`, `candlestick`, `book`) and
  routes through the unified `DataMessage` types.
- **`ExchangeConnector::response_for(&self, raw)`** trait method
  (default returns `None`). The runner calls it on every inbound text
  frame and forwards the returned text — used by Crypto.com to echo
  the server-initiated `public/heartbeat` with the matching `id`.

## [0.2.14] – 2026-05-27

### Added

- **Crypto.com private REST + signing** — `CryptocomPrivateClient` and
  `sign_cryptocom_request` implement Crypto.com's HMAC-SHA256 with the
  hex signature placed in the JSON body (`sig` field), distinct from
  KuCoin's header-based scheme and Kraken's HMAC-SHA512. 10 endpoints:
  account summary, order placement / cancellation, open orders,
  trades, deposit address, withdrawals.
- `build_params_string` — deterministic alphabetically-sorted
  parameter serialiser, public for callers building bespoke frames.

## [0.2.13] – 2026-05-26

### Added

- **Crypto.com public REST** (fourth envelope variant in the codebase)
  — `CryptocomRestClient`, six endpoints (`get_instruments`,
  `get_orderbook`, `get_candlestick`, `get_ticker`, `get_recent_trades`,
  `get_valuations`) + `unwrap_cryptocom_envelope<T>`.

## [0.2.12] – 2026-05-25

### Added

- **Kraken WebSocket v2 connector** — `KrakenConnector::public()` /
  `::private()` plus subscription helpers for `trade`, `ticker`,
  `ohlc`, `book` channels.

## [0.2.11] – 2026-05-25

### Added

- **Kraken private REST + HMAC-SHA512 signing** —
  `KrakenPrivateClient` and `sign_kraken_request`. 10 endpoints
  (balance, open/closed orders, place/cancel/cancel-all, trades
  history, ledger, withdraw, withdrawal status). Monotonic nonce with
  `AtomicU64` floor for concurrency safety.

## [0.2.10] – 2026-05-25

### Added

- **Kraken public REST** — `KrakenRestClient` covers system status,
  assets, asset pairs, ticker, depth, OHLC, recent trades, spread.
  Third envelope variant in the codebase.

## [0.2.9] – 2026-05-25

### Added

- **KuCoin `WsOrderClient`** — long-lived WebSocket connection to
  `wsapi.kucoin.com` with `clientOid`-based request/response
  correlation for low-latency order placement and cancellation.
  Shares the rate-limit guard with the data-feed runner via the
  newly `pub(crate)` `WsMsgGuard`.

## [0.2.8] – 2026-05-25

### Added

- **KuCoin spot-margin orders** (`src/rest/margin.rs`) —
  `place_margin_order`, `get_margin_order`, `cancel_margin_order`,
  `get_margin_fills`, `get_margin_balance` (v1 legacy).

## [0.2.7] – 2026-05-25

### Added

- **KuCoin Unified Trade Account REST** — `get_uta_account_summary`,
  `get_cross_margin_accounts`, `get_isolated_margin_accounts` with
  typed response structs (handles KuCoin's
  inconsistent-case `unrealisedPNLTotal` field).

## [0.2.6] – 2026-05-25

### Added

- **Bybit WebSocket connector** — `BybitConnector` with subscribe
  helpers for `publicTrade`, `tickers`, `kline`, `orderbook`.
- **`ExchangeConnector::ping_message(&self)`** trait method (default
  returns `None`) so each connector can supply its own application-
  ping format. KuCoin override returns `{"type":"ping"}`, Bybit
  returns `{"op":"ping"}`, Binance / Kraken stay `None`.

## [0.2.5] – 2026-05-25

### Added

- **Bybit public REST** (v5 unified API) — `BybitRestClient` covers
  kline, orderbook, tickers, recent trades, instruments, funding
  history, open interest, long/short ratio across spot / linear /
  inverse categories.

## [0.2.4] – 2026-05-24

### Added

- **Binance WebSocket connector** — `BinanceConnector::spot` /
  `::futures` with helpers for `aggTrade`, `bookTicker`, `kline`,
  `depth`, partial-depth, `markPrice` streams. URL-encoded stream
  subscription (no JSON subscribe frames).

## [0.2.3] – 2026-05-24

### Added

- **Binance public REST** — `BinanceRestClient` covers six spot
  endpoints (klines, depth, recent trades, book ticker, 24h ticker,
  exchange info) plus four USDT-M futures endpoints (klines,
  funding rate, premium index, open interest).

## [0.2.2] – 2026-05-24

### Added

- **`DataMessage::Candle`** and **`DataMessage::FundingRate`**
  variants on the unified `DataMessage` enum, with corresponding
  `CandleData` and `FundingData` structs. Foundation for the new
  exchange WS connectors. The enum is `#[non_exhaustive]` so this
  is additive.

## [0.2.1] – 2026-05-24

### Added

- **`PublicRestClient`** (`src/http.rs`) — unauthenticated HTTP
  layer with `reqwest::Client` (rustls + 10 s default timeout),
  jittered exponential-backoff retry, and `Retry-After` honoring on
  HTTP 429. Foundation for non-KuCoin REST integrations.

### Changed

- Shared helpers (`percent_encode`, `build_query_string`,
  `jitter_secs`) moved from `src/client.rs` to `src/http.rs` as
  `pub(crate)`; both `KuCoinClient` and `PublicRestClient` import
  them so they can't drift.

## [0.2.0] – 2026-05-24

### Changed (BREAKING)

- **Tighter `WsRunnerConfig` defaults** — `max_reconnect_attempts`
  dropped from 10 → 5, `max_reconnect_delay_secs` from 80 → 30.
  Worst-case reconnect window: ~9 min → ~95 s. Direct `run_feed`
  callers wanting the old behaviour should override:
  ```rust
  WsRunnerConfig {
      max_reconnect_attempts: 10,
      max_reconnect_delay_secs: 80,
      ..Default::default()
  }
  ```
  Supervised callers (`run_feed_supervised`) are unaffected.

## [0.1.14] – 2026-05-24

### Added

- **`RunnerEvent` observability hook** — `WsRunnerConfig.on_event:
  Option<EventListener>` lets callers track `SessionEnded`,
  `ReconnectsExhausted`, `TokenRefresh`, and `RefreshExhausted`
  events without log scraping. Designed for hourly reconnect-count
  metrics dashboards.

## [0.1.13] – 2026-05-24

### Added

- **`WsRunnerConfig.connect_timeout_secs`** (default 10) wrapping
  `connect_async` to bound stalled TLS / HTTP-upgrade handshakes.
- **`WsRunnerConfig.idle_timeout_secs`** (default 60) piggybacking
  on the ping tick to detect half-closed TCP connections via
  `last_frame_at` tracking.

## [0.1.12] – 2026-05-24

### Changed

- First session-end event of a new reconnect chain (attempt == 0 AND
  sub-5 s uptime) now logs at **WARN** with the close-frame code and
  reason — the canonical cascade signature from production-log
  analysis. Normal long-uptime rotations still log at INFO.

## [0.1.11] – 2026-05-24

### Added

- **`run_feed_supervised`** — wraps `run_feed` with a caller-supplied
  token-refresh closure called when the inner reconnect budget is
  exhausted (instead of returning `WsDisconnected`). Drops typical
  stale-token cascade blackout from ~9 min to ~10 s. New types:
  `SupervisedConfig`, `WsFeedEndpoint`.

## [0.1.x and earlier]

Initial KuCoin Futures REST + WebSocket implementation, including:

- HMAC-SHA256 request signing (key version 2)
- Generic `KuCoinClient` with jittered exponential-backoff retry and
  HTTP 429 `gw-ratelimit-reset` handling
- KuCoin's `{"code":"200000","data":{…}}` envelope unwrap
- 30+ REST endpoints across account, market data, orders, stop
  orders, fills, transfers
- `run_feed` runner: WS connect, ping, exponential-backoff reconnect,
  shutdown via `tokio::sync::watch`
- `KucoinConnector` covering all major futures channels (trade,
  ticker, orderbook depth + L2 delta, order updates, position
  changes, balance updates, advanced stop-order updates,
  instrument events)
- Bullet-public / bullet-private WS token negotiation
- 100 msg / 10 s sliding-window outbound rate limit

[Unreleased]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.19...HEAD
[0.2.19]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.18...v0.2.19
[0.2.18]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.17...v0.2.18
[0.2.17]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.16...v0.2.17
[0.2.16]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.15...v0.2.16
[0.2.15]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.14...v0.2.15
[0.2.14]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.13...v0.2.14
[0.2.13]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.12...v0.2.13
[0.2.12]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.11...v0.2.12
[0.2.11]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.10...v0.2.11
[0.2.10]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.9...v0.2.10
[0.2.9]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.8...v0.2.9
[0.2.8]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/nuniesmith/exchange-apiws/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/nuniesmith/exchange-apiws/compare/v0.1.14...v0.2.0
[0.1.14]: https://github.com/nuniesmith/exchange-apiws/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/nuniesmith/exchange-apiws/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/nuniesmith/exchange-apiws/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/nuniesmith/exchange-apiws/compare/v0.1.10...v0.1.11
