8.2 exchange-apiws

✓  Strengths
• Clean separation of REST vs WS
• run_feed handles TLS, ping, exponential-backoff reconnect automatically
• auth.rs HMAC signing has unit tests
• WsRunnerConfig is well-designed for flexible timeout configuration
• REST mock integration tests (wiremock) cover balance, positions, orders,
  fills, calc_contracts, envelope unwrapping, and HTTP error paths

✓  Completed
• contract_value fallback bug — calc_contracts now errors explicitly via
  ExchangeError::Order when multiplier is None; covered by
  calc_contracts_errors_when_multiplier_is_missing test
• get_open_stop_orders — implemented in rest/orders.rs with StopOrderDetail
  type; re-exported from rest/mod.rs
• Stop order test coverage — added to tests/rest_mock.rs:
    - get_open_stop_orders (happy path + empty list)
    - place_stop_order (stop-market + stop-limit)
    - cancel_stop_order
    - cancel_all_stop_orders (with results + no open stops)
    - get_done_orders (filled orders + empty list)
    - get_order by ID (happy path + not-found error path)
• Transfer endpoint tests — transfer_to_main, transfer_to_futures, and
  get_transfer_list covered in tests/rest_mock.rs (happy path + error paths,
  empty list, max_count cap enforcement)
• Account endpoint tests — added to tests/rest_mock.rs:
    - set_auto_deposit (true/false happy paths + api error propagation)
    - set_risk_limit_level (happy path + api error propagation)
    - add_position_margin (IN and OUT directions)
    - get_account_overview_all (multi-currency + empty list)
• WS integration test harness — tests/ws_types.rs spins up a local
  tokio-tungstenite server per test scenario; covers:
    - run_feed_delivers_messages
    - run_feed_shuts_down_cleanly
    - run_feed_reconnects_on_disconnect
    - run_feed_exhausts_reconnects_returns_error
• Duplicate test sections removed — rest_mock.rs had two passes of
  set_auto_deposit, set_risk_limit_level, and get_account_overview_all
  tests; first-pass (thinner) duplicates removed, keeping the complete
  second-pass versions with error-propagation coverage.

✓  Disconnection hardening (May 2026 — log analysis driven)

Root cause from 16 days of bot logs (May 8–24): every 10-attempt
exhaustion cascade was a stale-token symptom — sessions accepted, then
closed within ~1 s of subscribe. A freshly negotiated token recovered
on the very next attempt. The bare runner was burning 9+ minutes of
blackout per cascade because the token refresh lived only in the bot's
outer wrapper, gated behind WsDisconnected.

• Fix 1 — run_feed_supervised + SupervisedConfig + WsFeedEndpoint
  (src/ws/runner.rs). Wraps run_feed in an outer loop that calls a
  caller-supplied refresh closure when the inner reconnect budget is
  exhausted, instead of returning WsDisconnected.
    - Default SupervisedConfig: per-cycle attempts = 3 (down from 10),
      max_refresh_cycles = u32::MAX, refresh_delay_secs = 5.
    - Cycle-exhaustion → 5 s pause → refresh closure → new cycle.
    - Shutdown signal honoured both inside run_feed and during the
      refresh-delay wait via tokio::select on the shared watch::Receiver.
    - Refresh-closure errors propagate unchanged to the caller.
  Wired up in src/ws/mod.rs and src/lib.rs re-exports; documented with
  a runnable doctest. README has a "Supervised WebSocket feed" section
  with a full KuCoin example.
• Supervised test coverage — tests/ws_types.rs, 5 new tests covering:
    - supervised_refreshes_on_inner_exhaustion (refresh call counting)
    - supervised_recovers_with_new_endpoint (URL handoff on recovery)
    - supervised_propagates_refresh_error (Auth error surfaces)
    - supervised_exhausts_refresh_cycles (max_refresh_cycles = 0)
    - supervised_shuts_down_during_refresh (shutdown wins over delay)

• Fix 2 — promote first session-end reason to WARN with close-frame
  reason (src/ws/runner.rs). The Close(frame) and stream-None arms now
  fire WARN with close code + reason when attempt == 0 AND uptime < 5 s
  (the canonical cascade signature from the log analysis). Normal token
  rotations (long-lived session ending) still log at INFO, so the new
  WARN is a clean cascade-only signal for production log filters.
    - New const CASCADE_DETECT_SECS = 5
    - New helper const fn is_cascade_start(attempt, uptime_secs)
    - subscribed_at: Instant captured after subscribe loop completes
    - Err(read error) arm unchanged — existing attempt-based gate is
      correct for transient TCP resets; cascades manifest as Close
      frames in the bot's actual logs, not as read errors.
    - 3 unit tests in src/ws/runner.rs::tests covering the truth table.

• Fix 4 — connect & idle timeouts (src/ws/runner.rs). Two new
  WsRunnerConfig fields:
    - connect_timeout_secs (default 10): wraps connect_async in
      tokio::time::timeout so a stalled TLS or HTTP-upgrade handshake
      can't hang forever — surfaces as Disconnected within bound.
    - idle_timeout_secs (default 60): piggybacks on the ping tick to
      check `last_frame_at.elapsed()`. KuCoin sends a pong on each
      ping; ≥60 s of total silence ⇒ half-closed TCP ⇒ drop connection
      and let the reconnect path try again. Set to 0 to disable.
  Two new integration tests:
    - run_feed_connect_timeout_aborts_handshake — server reads HTTP
      upgrade but never replies; verifies bounded WsDisconnected.
    - run_feed_idle_timeout_drops_silent_connection — server accepts
      WS then goes silent; verifies idle path fires from the ping tick.

• Reconnect-event observability (src/ws/runner.rs). New types:
    - RunnerEvent enum (SessionEnded, ReconnectsExhausted, TokenRefresh,
      RefreshExhausted) — non_exhaustive so future variants don't break
      downstream match arms.
    - EventListener newtype wrapping Arc<dyn Fn(RunnerEvent)+Send+Sync>;
      Clone + manual Debug ("<callback>") so WsRunnerConfig still
      derives Debug+Clone cleanly.
    - on_event: Option<EventListener> on WsRunnerConfig; emit() helper
      is a no-op when None.
  Both run_feed and run_feed_supervised emit through the same listener
  so the bot can wire a single closure and get counts for both granular
  reconnects and cascade-triggered token refreshes (the Redis hourly
  counter use case).
  Two new integration tests in tests/ws_types.rs collect events through
  an Arc<Mutex<Vec<RunnerEvent>>> shared with the listener:
    - runner_emits_session_ended_and_exhausted_events
    - supervised_emits_token_refresh_and_exhausted_events

• Fix 3 — tighter bare-runner defaults (src/ws/runner.rs).
  WsRunnerConfig::default() updated from 10 × 80 s ≈ 9 min worst-case
  to 5 × 30 s ≈ 95 s worst-case. The old defaults were tuned for
  transient blips alone; cascades benefit from a faster bail so the
  caller's outer wrapper (typically run_feed_supervised) can refresh
  the token sooner. Breaking-ish behavior change for direct run_feed
  callers — they now surface WsDisconnected after ~1.5 min instead of
  ~9 min. Version bumped to 0.2.0 to signal the default change.
  SupervisedConfig::from_runner still overrides to 3 (now from 5,
  previously from 10) so supervised users get the even-tighter
  per-cycle ceiling.

⚠  Still open (architecture)
• Multi-exchange support — only KuCoin is implemented; the
  ExchangeConnector trait is the extension point for new venues.

# exchange-apiws — Roadmap

## Context

Canadian account — no Binance or Bybit API keys, but **all public REST and WS
endpoints are freely accessible**. KuCoin, Kraken, and Crypto.com API keys
are available for **spot only**.

---

## 1. Architecture prerequisites (do first)

Everything below depends on these two foundational pieces.

### 1a. PublicRestClient ✓ DONE

`KuCoinClient` calls `build_headers` on every request — it cannot make
unauthenticated calls. A new `PublicRestClient` is needed for Binance,
Bybit, and the public endpoints of all other exchanges.

Implemented in `src/http.rs`:
- `PublicRestClient::new(base_url)` (10 s default timeout) and
  `PublicRestClient::with_timeout(base_url, timeout)`.
- `get<T: DeserializeOwned>(path, params) -> Result<T>` — bare JSON,
  no envelope unwrapping (caller decides shape).
- Jittered exponential-backoff retry on network errors
  (DEFAULT_RETRIES = 3, DEFAULT_BACKOFF = 1.5 base).
- HTTP 429 honours the standard `Retry-After` seconds header (capped
  at MAX_RATE_LIMIT_RETRIES = 5 consecutive sleeps).
- 4xx/5xx → ExchangeError::Api without retry.

Shared helpers (percent_encode, build_query_string, jitter_secs) and
the retry tuning constants moved from client.rs to http.rs as
pub(crate); KuCoinClient now imports them. Single source of truth.

Tests: tests/public_rest_mock.rs (5 wiremock tests covering happy
path, query encoding, 4xx, 429 + Retry-After, retry-exhaustion).

Authenticated exchanges (Kraken, Crypto.com) will wrap this client and
add their own signing layer, rather than sharing KuCoinClient's KuCoin-
specific HMAC path.

### 1b. Envelope trait

Each exchange wraps responses differently:

| Exchange    | Envelope shape                                      |
|-------------|-----------------------------------------------------|
| KuCoin      | `{"code":"200000","data":{…}}`                      |
| Binance     | bare JSON — no wrapper                              |
| Bybit       | `{"retCode":0,"result":{…}}`                        |
| Kraken      | `{"result":{…},"error":[]}`                         |
| Crypto.com  | `{"code":0,"result":{…}}`                           |

Add a small `Envelope` trait (or free function) per exchange crate module
so each client can unwrap its own format and surface errors as
`ExchangeError::Api`.

### 1c. DataMessage additions ✓ DONE

New feed types that don't map to existing variants:

| Variant (new)      | Used by                                     |
|--------------------|---------------------------------------------|
| `Candle(CandleData)` | Binance kline stream, Bybit kline, Kraken OHLC, Crypto.com candlestick |
| `FundingRate(FundingData)` | Binance mark-price stream, Bybit ticker extended |

Added to `src/actors.rs`:
- `CandleData { symbol, exchange, interval, open_ts, open, high, low,
  close, volume, is_closed, receipt_ts }` — all f64 OHLCV + ms
  timestamps. `is_closed` distinguishes finalised bars from in-progress
  updates so consumers can filter.
- `FundingData { symbol, exchange, funding_rate, next_funding_time,
  mark_price: Option<f64>, index_price: Option<f64>, exchange_ts,
  receipt_ts }` — Optional mark/index because Binance's markPrice
  stream bundles them but Bybit's bare funding tick does not.
- `DataMessage::Candle(CandleData)` and `DataMessage::FundingRate(FundingData)`
  added to the `#[non_exhaustive]` enum so existing match arms still
  compile (forced catch-all).

KuCoin doesn't use these — its funding info already routes through
`DataMessage::InstrumentEvent` (subject `"funding.rate"`) and klines
are REST-only on KuCoin's contractMarket feed.

Tests in `src/actors.rs::tests`:
- candle_data_serde_round_trip
- funding_data_serde_round_trip_with_optionals
- funding_data_serde_round_trip_without_optionals
- data_message_new_variants_match (compile-time match smoke test)

---

## 2. KuCoin — remaining work

### 2a. Unified Trade Account (UTA) REST endpoints ✓ DONE

KuCoin Unified combines Spot + Futures margin in one account.
Base URL: `https://api.kucoin.com` (same as Spot — routed via the
existing `KucoinEnv::Unified` variant).

Implemented in `src/rest/uta.rs` as new methods on `KuCoinClient`:

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_uta_account_summary()` | `GET /api/v3/account/summary` | `UtaAccountSummary` |
| `get_cross_margin_accounts()` | `GET /api/v3/margin/accounts` | `CrossMarginAccount` (incl. per-currency `assets`) |
| `get_isolated_margin_accounts()` | `GET /api/v1/isolated/accounts` | `IsolatedMarginAccount` (incl. per-pair base/quote) |

Wire-format notes captured:
- `unrealisedPNLTotal` uses uppercase "PNL" — explicit `#[serde(rename)]`
  override (default camelCase would produce `unrealisedPnlTotal`).
- `maxBorrowSize` is omitted on some account states — typed as
  `Option<f64>` so the bare-borrow case round-trips cleanly.
- Boolean enable-flags (`borrowEnabled`, `transferInEnabled`, …) use
  `#[serde(default)]` so KuCoin's permissive empty-state responses
  parse without error.

Tests:
- 4 unit tests in `src/rest/uta.rs::tests` covering serde shapes,
  optional `max_borrow_size`, and the nested isolated-pair structure.
- 4 wiremock integration tests in `tests/rest_mock.rs` — one per
  endpoint plus a `411100 "UTA not enabled"` Api-error propagation
  test.

### 2b. Spot margin orders ✓ DONE

Implemented in `src/rest/margin.rs` as new methods on `KuCoinClient`.
Sizes/prices are base-asset string units on the wire (`"0.01"` BTC,
not the integer contract counts the futures API uses); typed responses
keep them as `String` and expose `_f64()` helpers for parse-on-demand.

| Method | Endpoint | Returns |
|--------|----------|---------|
| `place_margin_order(symbol, side, type, size, price, MarginModel, auto_borrow, tif)` | `POST /api/v1/margin/order` | `MarginOrderResponse` (orderId + borrowSize + loanApplyId) |
| `get_margin_order(order_id)` | `GET /api/v1/margin/orders/{id}` | `MarginOrderDetail` |
| `cancel_margin_order(order_id)` | `DELETE /api/v1/margin/orders/{id}` | `CancelMarginOrderResponse` (empty list if already done) |
| `get_margin_fills(symbol)` | `GET /api/v1/margin/fills` | `Vec<MarginFill>` (unwraps the `items` page) |
| `get_margin_balance()` | `GET /api/v1/margin/account` | `MarginAccountV1` (legacy — prefer v3 cross/isolated for new code) |

New types:
- `MarginModel { Cross, Isolated }` enum encoding the `marginModel` field.
- `MarginOrderResponse` — orderId + optional borrowSize / loanApplyId.
- `MarginOrderDetail` — full lifecycle state with `is_active` /
  `cancel_exist` flags and `size_f64()` / `price_f64()` parse helpers.
- `CancelMarginOrderResponse` — typed `cancelled_order_ids` list.
- `MarginFill` — single-fill detail (string-encoded numerics).
- `MarginAccountV1` + `MarginAccountAssetV1` — legacy v1 account shape.

Tests:
- 7 unit tests in `src/rest/margin.rs::tests` covering all response
  shapes (with/without borrow, helper parsers, empty cancel list,
  account assets, wire-format enum).
- 7 wiremock integration tests in `tests/rest_mock.rs` — one per endpoint,
  the "already done" empty-cancel case, the items-page unwrap pattern,
  and an Api-error propagation test for `place_margin_order`.

### 2c. WebSocket order placement ✓ DONE

KuCoin's `wsapi.kucoin.com` supports placing and cancelling orders over WS
for ultra-low latency. Implemented in `src/ws/orders.rs` as
`WsOrderClient` with a request/response correlation pipeline.

- `WsOrderClient::connect(ws_url)` opens the WS, spawns a writer task
  (drains an mpsc with the shared `WsMsgGuard` between sends) and a
  reader task (routes inbound frames to pending `oneshot::Sender`s via
  `clientOid` lookup in an `Arc<Mutex<HashMap<...>>>`).
- `place_order(symbol, side, size, leverage, type, price)` builds the
  KuCoin frame (`{type:"openOrder", topic:"/contractMarket/order",
  data:{clientOid, side, symbol, type, size, leverage, price?}}`),
  pushes it to the outbound queue, awaits the matching `WsOrderAck`.
- `cancel_order(order_id)` follows the same shape with `cancelOrder`.
- `send_raw(client_oid, frame)` is the escape hatch for callers that
  need to construct a frame the canonical helpers don't model.
- 5 s default per-request timeout; tunable via
  `.with_request_timeout(d)`. Pending entries are cleaned up on
  timeout so a late ack doesn't linger.
- `close()` tears the connection down; any still-pending requests get
  a `connection_closed` sentinel `WsOrderAck` so callers don't hang.
- Rate-limit guard from runner.rs is now `pub(crate) WsMsgGuard` —
  same 100 msg/10s sliding window shared between the data-feed runner
  and the order client (per-connection, not per-account).

Wire format caveat: KuCoin's wsapi schema isn't as exhaustively
documented as the public feed; the request/response shape used here
matches current public docs but may need adjustment if the schema
diverges. `build_place_order_frame` / `build_cancel_order_frame` are
public so callers can copy the canonical shape and route a tweaked
version via `send_raw`.

Tests:
- 8 unit tests in `src/ws/orders.rs::tests` covering frame builders
  (limit + market), inbound parser (`ack`, `error`, `welcome`, `pong`,
  missing `clientOid`).
- 5 integration tests in `tests/ws_orders_mock.rs` spinning up a local
  tokio-tungstenite server: round-trip, **concurrent out-of-order
  routing** (3 in-flight, server replies in reverse), error frame,
  request timeout, connection-close pending-cleanup.

---

## 3. Binance — public only

No API keys. All endpoints below are unauthenticated.

### 3a. REST (`src/binance/rest.rs`) ✓ DONE

Uses `PublicRestClient` pointed at:
- Spot: `https://api.binance.com`
- Futures (USDT-M): `https://fapi.binance.com`

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_klines(symbol, interval, limit)` | `GET /api/v3/klines` | `Vec<BinanceKline>` |
| `get_orderbook(symbol, limit)` | `GET /api/v3/depth` | `BinanceOrderBook` |
| `get_recent_trades(symbol, limit)` | `GET /api/v3/trades` | `Vec<BinanceTrade>` |
| `get_ticker(symbol)` | `GET /api/v3/ticker/bookTicker` | `BinanceBookTicker` |
| `get_ticker_24h(symbol)` | `GET /api/v3/ticker/24hr` | `BinanceTicker24h` |
| `get_exchange_info()` | `GET /api/v3/exchangeInfo` | `serde_json::Value` (filter shape varies by symbol) |
| `get_futures_klines(symbol, interval, limit)` | `GET /fapi/v1/klines` | `Vec<BinanceKline>` |
| `get_futures_funding_rate(symbol, limit)` | `GET /fapi/v1/fundingRate` | `Vec<BinanceFundingRate>` |
| `get_futures_mark_price(symbol)` | `GET /fapi/v1/premiumIndex` | `BinanceMarkPrice` |
| `get_futures_open_interest(symbol)` | `GET /fapi/v1/openInterest` | `BinanceOpenInterest` |

Response: bare JSON arrays/objects (no envelope wrapper). The
`BinanceKline` shape needs a custom Deserialize (Binance returns a
12-element heterogeneous array) — implementation in src/binance/rest.rs.

`BinanceKline::into_candle_data(symbol, interval)` and
`BinanceFundingRate::into_funding_data()` bridge raw Binance responses
to the unified `CandleData` / `FundingData` types from §1c — same
downstream code path as any other exchange.

Tests:
- 5 unit tests in `src/binance/rest.rs::tests` covering the kline
  array Deserialize, into_candle_data, orderbook level parsing, book
  ticker shape, and funding-rate → FundingData bridge.
- 10 wiremock integration tests in `tests/binance_rest_mock.rs` —
  one per endpoint, end-to-end via a local mock HTTP server.

### 3b. WebSocket (`src/binance/ws.rs`) ✓ DONE

Implements `ExchangeConnector`. Binance WS uses URL-encoded stream names
rather than subscription messages after connect — `subscription_message`
returns `None` and the full WSS URL is built at connector construction.

Constructors (`BinanceConnector::spot(&[&str])`, `::futures(&[&str])`)
always use the combined-stream endpoint (`/stream?streams=…`) so a
single connection multiplexes many topics; `parse_message` unwraps the
`{"stream":…,"data":…}` envelope.

| Helper (static fn → String) | Stream | DataMessage |
|---------------------|--------|-------------|
| `trade_stream(symbol)` | `<sym>@aggTrade` | `Trade` |
| `ticker_stream(symbol)` | `<sym>@bookTicker` | `Ticker` (best_bid/ask; no last price) |
| `kline_stream(symbol, interval)` | `<sym>@kline_<interval>` | `Candle` (is_closed flag) |
| `depth_stream(symbol)` | `<sym>@depth@100ms` | `OrderBook` (delta) |
| `depth_snapshot_stream(symbol, levels)` | `<sym>@depth{5\|10\|20}@100ms` | `OrderBook` (snapshot) |
| `mark_price_stream(symbol)` (futures) | `<sym>@markPrice@1s` | `FundingRate` (mark + index) |

Symbol case-handling: helpers lowercase symbols automatically per
Binance's URL convention. The trade `m` field maps `m=true` →
TradeSide::Sell (buyer is maker = aggressive sell).

The partial-depth snapshot (no `e` field, no symbol in the frame)
gets its symbol from the combined-stream wrapper's `stream` key.

Ping: Binance sends protocol-level WS Ping frames every ~3 min; the
runner responds with Pong automatically. No application-level JSON ping
required — `ping_interval_secs` is 180 in the default config (unused
in practice but kept above the idle-timeout threshold).

Tests:
- 11 unit tests in `src/binance/ws.rs::tests` — one per variant +
  helper string format + URL construction + unknown-event handling.
- 1 integration test in `tests/binance_ws_mock.rs` that spins up a
  local tokio-tungstenite server, pushes one frame of each Binance
  type (Trade, Ticker, Candle, depth delta, depth snapshot, Funding),
  and asserts the runner + connector deliver all six variants
  through `run_feed`.

---

## 4. Bybit — public only

No API keys. All endpoints below are unauthenticated.

### 4a. REST (`src/bybit/rest.rs`) ✓ DONE

Uses `PublicRestClient` pointed at `https://api.bybit.com`.

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_klines(category, symbol, interval, limit)` | `GET /v5/market/kline` | `BybitListResult<BybitKline>` |
| `get_orderbook(category, symbol, limit)` | `GET /v5/market/orderbook` | `BybitOrderBook` |
| `get_tickers(category, Option<symbol>)` | `GET /v5/market/tickers` | `BybitListResult<BybitTicker>` |
| `get_recent_trades(category, symbol, limit)` | `GET /v5/market/recent-trade` | `BybitListResult<BybitTrade>` |
| `get_instruments(category)` | `GET /v5/market/instruments-info` | `serde_json::Value` |
| `get_funding_rate(category, symbol, limit)` | `GET /v5/market/funding/history` | `BybitListResult<BybitFundingRate>` |
| `get_open_interest(category, symbol, interval_time, limit)` | `GET /v5/market/open-interest` | `BybitListResult<BybitOpenInterest>` |
| `get_long_short_ratio(category, symbol, period, limit)` | `GET /v5/market/account-ratio` | `BybitListResult<BybitLongShortRatio>` |

`BybitCategory` enum encodes `"spot"`, `"linear"`, `"inverse"`. The
envelope `{"retCode":N,"result":...,"retMsg":...}` is unwrapped by a
free function `unwrap_bybit_envelope<T>` exposed for external use too —
non-zero `retCode` surfaces as `ExchangeError::Api` with the code and
`retMsg` preserved.

Bybit's wire format is mostly stringified — kline rows are positional
string arrays (custom Deserialize), most timestamps come as JSON
strings (`str_i64` adapter), prices/qtys as strings (`str_f64`).
`opt_str_f64` handles fields that vary across product classes (Spot
omits funding/mark/index fields).

Bridges:
- `BybitKline::into_candle_data(symbol, interval)` → unified `CandleData`
- `BybitFundingRate::into_funding_data()` → unified `FundingData`
  (mark/index both `None` since this endpoint doesn't bundle them).

Tests:
- 7 unit tests in `src/bybit/rest.rs::tests` covering envelope unwrap
  (success + Api-error), kline array Deserialize, into_candle_data,
  orderbook short-key shape, category wire format, funding bridge.
- 9 wiremock integration tests in `tests/bybit_rest_mock.rs` — one per
  endpoint plus an error-envelope propagation test.

### 4b. WebSocket (`src/bybit/ws.rs`) ✓ DONE

Implements `ExchangeConnector`. Bybit's WS is subscribe-after-connect:
`BybitConnector::new(category, topics)` packages topics into a single
`{"op":"subscribe","args":[…]}` frame returned by `subscription_message`
and sent immediately after handshake. `ping_message` returns the
Bybit-specific `{"op":"ping"}` (server replies `{"op":"pong"}`).

URL per category:
- Spot:    `wss://stream.bybit.com/v5/public/spot`
- Linear:  `wss://stream.bybit.com/v5/public/linear`
- Inverse: `wss://stream.bybit.com/v5/public/inverse`

| Helper (static fn → String) | Topic | DataMessage |
|-----------------------------|-------|-------------|
| `trade_topic(symbol)` | `publicTrade.<sym>` | `Trade` (one per array element — Bybit batches) |
| `ticker_topic(symbol)` | `tickers.<sym>` | `Ticker` (snapshot/delta type flag) |
| `kline_topic(symbol, interval)` | `kline.<interval>.<sym>` | `Candle` (`confirm` = closed) |
| `orderbook_topic(symbol, depth)` | `orderbook.<depth>.<sym>` | `OrderBook` (snapshot then deltas) |

Trait extension supporting this PR:
- `ExchangeConnector::ping_message(&self) -> Option<String>` added with
  default returning `None` (matches Binance — no app ping, protocol Ping
  only). Each connector that needs an app ping overrides:
    - KucoinConnector → `{"type":"ping"}`
    - BybitConnector → `{"op":"ping"}`
    - BinanceConnector inherits the `None` default.
- The runner now calls `connector.ping_message()` instead of the
  hard-coded KuCoin format. Idle check still fires regardless; if
  `ping_message` returns `None` the tick simply doesn't send anything.

Tests:
- 12 unit tests in `src/bybit/ws.rs::tests` covering subscribe-frame
  shape, empty-topics behaviour, ping format, op-ack and pong
  passthrough, all four parser paths (trade-batch, ticker, kline,
  orderbook snapshot+delta), and per-category URL routing.
- 1 integration test in `tests/bybit_ws_mock.rs` that spins up a local
  WS server, captures the subscribe frame + ping JSON, and asserts
  Trade/Ticker/Candle/OrderBook all flow through `run_feed`.

---

## 5. Kraken — spot, authenticated

API keys available. Signing: HMAC-SHA512 over
`URI + SHA256(nonce + encoded_body)`, base64-encoded, sent as
`API-Sign` header alongside `API-Key`.

Add `src/kraken/auth.rs` (separate from `src/auth.rs` which is KuCoin-
specific).

### 5a. Public REST (`src/kraken/rest.rs`) ✓ DONE

Base URL: `https://api.kraken.com`. Uses `PublicRestClient`; the
Kraken envelope `{"result":...,"error":[]}` is unwrapped by a free
function `unwrap_kraken_envelope<T>` (public for external use). A
non-empty `error` array surfaces as `ExchangeError::Api` with all
messages joined by `"; "`.

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_system_status()` | `/0/public/SystemStatus` | `KrakenSystemStatus` |
| `get_assets()` | `/0/public/Assets` | `HashMap<String, KrakenAsset>` |
| `get_asset_pairs(pair?)` | `/0/public/AssetPairs` | `HashMap<String, KrakenAssetPair>` |
| `get_ticker(pair)` | `/0/public/Ticker` | `HashMap<String, KrakenTicker>` |
| `get_orderbook(pair, count)` | `/0/public/Depth` | `HashMap<String, KrakenOrderBook>` |
| `get_ohlc(pair, interval_mins)` | `/0/public/OHLC` | `serde_json::Value` (mixed pair+"last" shape) |
| `get_recent_trades(pair)` | `/0/public/Trades` | `serde_json::Value` (same) |
| `get_spread(pair)` | `/0/public/Spread` | `serde_json::Value` (same) |

Kraken-specific wire-format notes captured:
- OHLC/Trades/Spread mix per-pair arrays with a `"last"` cursor key
  at the same level — surfaced as `serde_json::Value` so callers can
  pick whichever shape suits them (paginate via `last`, iterate
  per-pair). A typed custom-Deserialize wrapper can be added later
  if there's demand.
- KrakenTicker fields use tuple wire shapes (`a/b/c/v/p/t/l/h`);
  the struct stores them as-is and exposes `ask_price()`, `bid_price()`,
  `last_price()`, `volume_24h()`, `high_24h()`, `low_24h()` helpers.
- KrakenOrderBook levels are `(price_str, volume_str, timestamp_f64)`
  — Kraken sends price/volume as strings but the timestamp as a JSON
  number; the tuple type accommodates both. `bids_f64()` / `asks_f64()`
  drop the timestamp column for downstream cross-exchange routing.
- KrakenAsset / KrakenAssetPair use `#[serde(default)]` on optional
  fields (`collateral_value`, `status`, `wsname`) since Kraken omits
  them on inactive or non-collateral assets.

Tests:
- 6 unit tests in `src/kraken/rest.rs::tests` covering envelope
  unwrap (success + Api error with joined messages), ticker tuple
  helpers, orderbook helper, missing-optionals for Asset and AssetPair.
- 10 wiremock integration tests in `tests/kraken_rest_mock.rs` —
  one per endpoint (including both filtered and unfiltered
  AssetPairs) plus an error-envelope propagation test.

### 5b. Private REST (authenticated) ✓ DONE

New `src/kraken/auth.rs` introduces Kraken's HMAC-SHA512 signing
(distinct from the KuCoin HMAC-SHA256 in `src/auth.rs`). New
`src/kraken/private.rs` exposes `KrakenPrivateClient` with the 10
documented endpoints:

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_balance()` | `POST /0/private/Balance` | `HashMap<String, String>` |
| `get_open_orders()` | `POST /0/private/OpenOrders` | `KrakenOpenOrders` |
| `get_closed_orders()` | `POST /0/private/ClosedOrders` | `KrakenClosedOrders` (with `count`) |
| `place_order(pair, side, type, volume, price?)` | `POST /0/private/AddOrder` | `KrakenAddOrderResponse` |
| `cancel_order(txid)` | `POST /0/private/CancelOrder` | `KrakenCancelResponse` |
| `cancel_all_orders()` | `POST /0/private/CancelAll` | `KrakenCancelResponse` |
| `get_trades_history()` | `POST /0/private/TradesHistory` | `serde_json::Value` |
| `get_ledger(asset)` | `POST /0/private/Ledgers` | `serde_json::Value` |
| `withdraw(asset, key, amount)` | `POST /0/private/Withdraw` | `KrakenWithdrawResponse` |
| `get_withdrawal_status(asset)` | `POST /0/private/WithdrawStatus` | `serde_json::Value` |

Auth implementation notes:
- `KrakenCredentials { api_key, api_secret_b64 }` is `ZeroizeOnDrop`,
  loadable from `KRAKEN_API_KEY` / `KRAKEN_API_SECRET` env vars.
- `sign_kraken_request(uri_path, nonce, post_body, secret_b64)` does:
  `base64(HMAC_SHA512(decoded_secret, uri_path || SHA256(nonce_str || post_body)))`.
  Public so callers building bespoke requests can reuse it.
- `form_encode(&[(&str, &str)])` builds the
  `application/x-www-form-urlencoded` body (space → `+`, reserved
  chars → `%XX`); the body MUST match byte-for-byte what gets signed.
- Strictly monotonic nonces — initialised from millisecond wall
  clock; an `AtomicU64` floor (raised via `fetch_max` then incremented
  via `fetch_add`) guarantees uniqueness across concurrent calls and
  survives clock rewinds.

The 10 endpoints return typed structs where shape is predictable
(`KrakenOrder` and friends) and `serde_json::Value` for richer
multi-field payloads (`TradesHistory`, `Ledgers`, `WithdrawStatus`)
where callers usually want to pick specific fields. Typed responses
preserve numeric fields as `String` (Kraken's wire format) so
arbitrary precision is preserved.

Tests:
- 9 unit tests in `src/kraken/auth.rs::tests` — credentials round-trip,
  invalid-base64 rejection, signing determinism, signing
  sensitivity to every input (nonce/path/body/secret), output is
  88-char base64, form-encoding edge cases (empty, spaces,
  reserved chars).
- 4 unit tests in `src/kraken/private.rs::tests` — nonce
  monotonicity across 1000 calls, response-type serde shapes.
- 11 wiremock integration tests in `tests/kraken_private_mock.rs`
  — one per endpoint plus an error-envelope propagation test;
  asserts on `API-Key` / `API-Sign` headers + `nonce=` in body.

### 5c. WebSocket (`src/kraken/ws.rs`) ✓ DONE (public channels)

Implements `ExchangeConnector`. Kraken v2 is subscribe-after-connect
with one subscribe frame per channel (not bundled like Bybit's `args`
array); `subscription_message` returns `None` and the caller passes
the list of frames to `run_feed` as the `subscriptions` vector.

`ping_message` returns the Kraken-specific `{"method":"ping"}` —
trait extension landed in PR #11 makes this a clean override.

URLs:
- Public: `wss://ws.kraken.com/v2` ← [`KrakenConnector::public`]
- Private: `wss://ws-auth.kraken.com/v2` ← [`KrakenConnector::private`]

| Helper (static fn → String) | Channel | DataMessage |
|-----------------------------|---------|-------------|
| `trade_subscription(&[pairs])` | `trade` | `Trade` (one per element of `data` array) |
| `ticker_subscription(&[pairs])` | `ticker` | `Ticker` (snapshot/update both routed) |
| `ohlc_subscription(&[pairs], interval_mins)` | `ohlc` | `Candle` (always `is_closed = false` — Kraken v2 has no "closed" flag) |
| `book_subscription(&[pairs], depth)` | `book` | `OrderBook` (snapshot then deltas) |

Private channels (`executions`, `balances`) require a token from
`POST /0/private/GetWebSocketsToken`. Not wired up with typed helpers
in this PR — callers can subscribe via custom JSON over a
`KrakenConnector::private()` connection. The parser ignores unknown
channels rather than erroring so private subscriptions don't trip it.

Tests:
- 11 unit tests in `src/kraken/ws.rs::tests` covering URL routing,
  ping format, subscribe-frame shape, all four parsers, snapshot+delta
  for book, and the heartbeat / pong / subscribe-ack / unknown-channel
  passthrough paths.
- 1 integration test in `tests/kraken_ws_mock.rs` spinning up a local
  tokio-tungstenite server: pushes one frame of each channel,
  captures the runner's subscribe frames + ping JSON, asserts all
  four DataMessage variants flow through `run_feed`.

---

## 6. Crypto.com — spot, authenticated

API keys available. Signing: HMAC-SHA256 over a deterministic parameter
string. Sent as `sig` field in the request body (not a header).

Add `src/cryptocom/auth.rs`.

### 6a. Public REST (`src/cryptocom/rest.rs`) ✓ DONE

Base URL: `https://api.crypto.com/exchange/v1`. Uses
`PublicRestClient`; envelope `{"code": N, "result": {...}}` is
unwrapped by a free function `unwrap_cryptocom_envelope<T>` (public
for external use). Non-zero `code` → `ExchangeError::Api`. This is
the fourth envelope variant in the codebase after KuCoin / Bybit /
Kraken.

| Method | Endpoint | Returns |
|--------|----------|---------|
| `get_instruments()` | `/public/get-instruments` | `Vec<CryptocomInstrument>` |
| `get_orderbook(instrument, depth)` | `/public/get-book` | `CryptocomOrderBook` |
| `get_candlestick(instrument, timeframe)` | `/public/get-candlestick` | `Vec<CryptocomCandle>` |
| `get_ticker(Option<instrument>)` | `/public/get-ticker` | `Vec<CryptocomTicker>` |
| `get_recent_trades(instrument)` | `/public/get-trades` | `Vec<CryptocomTrade>` |
| `get_valuations(instrument, valuation_type)` | `/public/get-valuations` | `Vec<CryptocomValuation>` |

Wire-format quirks captured:
- Crypto.com uses single-letter field names (`i`, `b`, `k`, `vv`, `a`,
  `h`, `l`, `c`, `v`, `t`, `o`, `s`, `p`, `q`, `d`) — `#[serde(rename)]`
  on every struct field maps them to readable names. Best ask is `"k"`
  on the ticker (unintuitively).
- Most endpoints wrap their data in `result.data: Vec<T>`; an internal
  `DataList<T>` newtype handles the unwrap uniformly.
- Orderbook levels include a num-orders column — `bids_f64()` /
  `asks_f64()` drop it for cross-exchange-friendly output.
- `get_orderbook` returns `result.data[0]` as a single book (rather
  than `Vec<CryptocomOrderBook>`); an empty array surfaces as
  `Api(code="empty_data")`.
- `get_valuations` covers what the roadmap called `get_funding_rate`;
  the real endpoint serves multiple value types via the
  `valuation_type` query param (`"mark_price"`, `"index_price"`,
  `"funding_rate"`, `"estimated_funding_rate"`).
- Numeric fields stay `String` to preserve wire precision; `*_f64()`
  helpers convert on demand.

Tests:
- 8 unit tests in `src/cryptocom/rest.rs::tests` — envelope unwrap
  (success + Api error with `message`), orderbook helper, candle
  letter renaming + parse helpers, ticker letter renaming, trade and
  valuation deserialise, instrument with missing optionals.
- 9 wiremock integration tests in `tests/cryptocom_rest_mock.rs` —
  one per endpoint (including filtered and unfiltered ticker) plus
  Api-error propagation and the empty-book edge case.

### 6b. Private REST (authenticated)

| Method | Endpoint |
|--------|----------|
| `get_account_summary(currency)` | `POST /private/get-account-summary` |
| `place_order(instrument, side, type, quantity, price)` | `POST /private/create-order` |
| `cancel_order(order_id)` | `POST /private/cancel-order` |
| `cancel_all_orders(instrument)` | `POST /private/cancel-all-orders` |
| `get_open_orders(instrument)` | `POST /private/get-open-orders` |
| `get_order_detail(order_id)` | `POST /private/get-order-detail` |
| `get_trades(instrument)` | `POST /private/get-trades` |
| `get_deposit_address(currency)` | `POST /private/get-deposit-address` |
| `create_withdrawal(currency, amount, address)` | `POST /private/create-withdrawal` |
| `get_withdrawal_history(currency)` | `POST /private/get-withdrawal-history` |

### 6c. WebSocket (`src/cryptocom/ws.rs`)

Implements `ExchangeConnector`.

Public: `wss://stream.crypto.com/exchange/v1/market`
Private: `wss://stream.crypto.com/exchange/v1/user`

Ping: send `{"method":"public/heartbeat"}` every 30 s;
server responds with heartbeat — respond with `{"method":"public/respond-heartbeat","id":<same_id>}`.

Subscribe message:
```json
{"id":1,"method":"subscribe","params":{"channels":["book.BTC_USDT.10"]}}
```

| Subscription helper | Channel pattern | DataMessage |
|---------------------|-----------------|-------------|
| `trade_subscription(instrument)` | `trade.<instrument>` | `Trade` |
| `ticker_subscription(instrument)` | `ticker.<instrument>` | `Ticker` |
| `kline_subscription(instrument, timeframe)` | `candlestick.<tf>.<instrument>` | `Candle` |
| `orderbook_subscription(instrument, depth)` | `book.<instrument>.<depth>` | `OrderBook` |
| `order_updates_subscription()` ⚑ | `user.order.<instrument>` | `OrderUpdate` |
| `balance_subscription()` ⚑ | `user.balance` | `BalanceUpdate` |

⚑ Private — connect to the private WS URL with a signed auth frame
sent immediately after connect.

---

## Implementation order (suggested)

1. `PublicRestClient` + `Envelope` trait  
2. `DataMessage::Candle` + `DataMessage::FundingRate` variants  
3. Binance public REST + WS (simplest envelope, most documentation)  
4. Bybit public REST + WS  
5. KuCoin UTA + spot margin REST  
6. KuCoin WS order placement  
7. Kraken public REST + WS  
8. Kraken private REST + private WS  
9. Crypto.com public REST + WS  
10. Crypto.com private REST + private WS  

---

## File layout (target)

```
src/
├── http.rs              (new) PublicRestClient
├── binance/
│   ├── mod.rs
│   ├── rest.rs
│   └── ws.rs
├── bybit/
│   ├── mod.rs
│   ├── rest.rs
│   └── ws.rs
├── kraken/
│   ├── mod.rs
│   ├── auth.rs
│   ├── rest.rs
│   └── ws.rs
├── cryptocom/
│   ├── mod.rs
│   ├── auth.rs
│   ├── rest.rs
│   └── ws.rs
├── rest/               (existing — KuCoin)
│   ├── account.rs      + UTA endpoints
│   ├── margin.rs       (new) KuCoin spot margin
│   ├── market.rs
│   ├── mod.rs
│   └── orders.rs
└── ws/                 (existing — KuCoin)
    ├── orders.rs       (new) WS order placement
    └── …
tests/
├── binance_rest.rs     (new)
├── bybit_rest.rs       (new)
├── kraken_rest.rs      (new)
├── cryptocom_rest.rs   (new)
├── rest_mock.rs        (existing — extend for margin, UTA)
└── ws_types.rs         (existing)
```
Now let me read the actual runner source to audit it:Good — I can see the full README and the log patterns tell me everything I need to know about the runner's internals. Let me give you concrete fixes for all three issues across both codebases.

---

**What the logs reveal about the runner:**

- Reconnect backoff: 5 → 10 → 20 → 40 → 80 → 80×5s (10 attempts max), then `WS feed terminated — re-negotiating token delay=30s`
- `WS read error` fires at WARN for every reset — even the single-attempt ones that recover immediately
- April 16's 97 errors included a full exhaustion cycle at 08:28 AND a prolonged HTTP outage from ~13:45–14:32 where all three symbols' candle fetches were failing together — likely a brief KuCoin-side incident

---

**Fix 1 — `exchange-apiws`: reduce log noise from transient resets**

Right now every `Connection reset by peer` hits WARN immediately. Single-attempt recoveries should be DEBUG:

```rust
// In src/ws/runner.rs — wherever the read error arm is handled
Err(e) => {
    attempt += 1;
    if attempt == 1 {
        // Transient reset — KuCoin does this regularly, don't alarm on first hit
        tracing::debug!(exchange = %self.exchange_name(), error = %e, "WS read error (transient)");
    } else {
        tracing::warn!(exchange = %self.exchange_name(), error = %e, attempt, max = config.max_reconnects, "WS read error");
    }
    // ... existing backoff logic
}
```

This alone would have cut Apr 16's log entries from 97 down to maybe 10–15 — just the ones that actually needed multiple retries.

---

**Fix 2 — `exchange-apiws`: make `WsRunnerConfig` fields configurable**

The README shows `WsRunnerConfig::default()` but doesn't expose the fields. Based on the log output (max=10, delays 5/10/20/40/80) you want to at least expose:

```rust
#[derive(Debug, Clone)]
pub struct WsRunnerConfig {
    pub max_reconnects: u32,       // default: 10
    pub base_delay_secs: u64,      // default: 5
    pub max_delay_secs: u64,       // default: 80
    pub ping_interval_secs: u64,   // default: 18
    pub token_renegotiate_delay_secs: u64,  // default: 30 — this is the blind window after exhaustion
}
```

The `token_renegotiate_delay_secs` is the most important one for a trading context. 30s of total blindness after exhaustion is a long time. You could drop it to 5s for futures trading since you're not at risk of hammering the token endpoint.

3. **`exchange-apiws`** — demote single-attempt WS resets from WARN to DEBUG. Publish as `0.1.9`, update the bot's `Cargo.toml`.
4. **`exchange-apiws`** — expose `token_renegotiate_delay_secs` in `WsRunnerConfig` and drop it to 5s in your bot's config for the futures context.
5. **Bot** — add the hourly reconnect counter to Redis so you get early warning of connectivity trouble before it degrades into a full exhaustion cycle.
