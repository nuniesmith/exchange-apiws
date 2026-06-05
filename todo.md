# exchange-apiws — Roadmap

> Forward-looking backlog. The original build-out roadmap is **complete** —
> see the [Done](#done--original-roadmap-complete) summary at the bottom and
> `CHANGELOG.md` for the per-version detail.

## Where things stand (2026-06, **v0.5.0 — published on crates.io**)

Seven venues. Shared async runner with supervised token refresh,
connect/idle timeouts, cascade-start WARN, and a `RunnerEvent`
observability hook. Offline test suite (wiremock + local
`tokio-tungstenite`), all green. CI gates fmt / clippy (all-features **and**
no-default-features) / test matrix / rustdoc (`-D warnings`) / MSRV 1.94.1.
Per-exchange Cargo features. Runnable examples. README + CHANGELOG current.

| Exchange | Public REST | Private REST | Public WS | Private WS | WS order entry |
|---|---|---|---|---|---|
| KuCoin     | ✓ | ✓ | ✓ | ✓ | ✓ |
| Binance    | ✓ | — | ✓ | **✓ (0.6.0)** | — |
| Bybit      | ✓ | **✓ (0.4.0)** | ✓ | **✓ (0.6.0)** | — |
| Kraken     | ✓ | ✓ | ✓ | **✓ (0.6.0)** | — |
| Crypto.com | ✓ | ✓ | ✓ | **✓ (0.6.0)** | — |
| Coinbase   | — | — | **✓ (0.5.0)** | — | — |
| OKX        | — | — | **✓ (0.5.0)** | — | — |

**Recently shipped** (consumed by the FKS stack — see
`fks-full/docs/MULTI_ASSET_BRAIN_ROADMAP.md`):
- **0.4.0** — signed Bybit v5 REST (`BybitPrivateClient`: orders, positions,
  wallet) + HMAC signing. Closed the "Bybit has no signed surface" gap.
- **0.5.0** — Coinbase Advanced Trade + OKX v5 **public WS connectors**
  (trades/tickers/books → unified `DataMessage`).
- **0.6.0** — **private WS user-data** for Bybit, Binance, Crypto.com & Kraken
  (orders/fills/balances → `OrderUpdate` / `BalanceUpdate`), so every venue with
  a private WS is now account-aware for janus's Bybit-compat path. Plus the
  breaking `OrderUpdate`/`AdvancedOrderUpdate` quantity widening to `f64` (no
  more fractional-size truncation).

**Remaining gaps the matrix shows:** Binance still has no signed account/order
REST; Coinbase/OKX are WS-only (no REST/private); only KuCoin places orders over
WS — **C5** is the last open private-side item. Those columns are the functional
roadmap (Sections B/C/H below).

---

## Recommended next steps (priority order)

> ✅ **Published.** The crate is live on crates.io (0.5.0). `release.sh` is
> publish-safe and used. The priority list below now leads with functional
> surface, not the publish.

1. ~~**Other venues' private WS user-data streams.**~~ **Done** — Bybit (C3),
   Binance (C2), Crypto.com (C4), and Kraken (C1) all stream order/fill +
   balance events into `OrderUpdate` / `BalanceUpdate`, so every venue with a
   private WS is now account-aware for the FKS brain / janus's `bybit_compat`.
   Remaining private-side work is **WS order *entry*** beyond KuCoin (C5).
   → [C](#c-private-websocket--ws-order-entry)

2. **Binance private REST** (signed account / orders /
   positions). They're read-only today; for a *published* crate this is
   the most-requested surface. Binance = HMAC-SHA256 over the query
   string; Bybit v5 = HMAC-SHA256 over `timestamp+key+recv_window+body`
   in headers. → [B1](#b-private-trading-surface), [B2](#b-private-trading-surface)

3. **Private WebSocket user-data streams** for the four venues missing
   them. This is what makes a feed *trade-aware* (your own fills,
   positions, balances) instead of just market data. → [C](#c-private-websocket--ws-order-entry)

4. **Local order-book maintainer.** Connectors already emit depth
   snapshots + deltas, but there's no helper to assemble a synchronized
   book with sequence-gap detection and snapshot resync. Without it the
   depth streams aren't directly usable. → [D1](#d-market-data-quality)

5. **CI supply-chain + semver gates** (`cargo-deny`, `cargo-audit`,
   `cargo-semver-checks`, coverage). Cheap to add, protects every
   release from here on. → [F](#f-quality-ci--supply-chain)

---

## A. Publishing & release

- [ ] **A1 — Publish 0.3.x to crates.io.** Run `./scripts/release.sh`
      (gates: clean tree, no existing tag, CHANGELOG section, build,
      test, `cargo package`). Requires `CARGO_REGISTRY_TOKEN` /
      `cargo login`. _Blocked on your token — I can prep + dry-run it._
- [ ] **A2 — `package.metadata.docs.rs`** in `Cargo.toml` so docs.rs
      builds with `--all-features` (otherwise the optional exchanges are
      missing from the published docs). Add
      `rustdoc-args = ["--cfg", "docsrs"]` and `all-features = true`.
- [ ] **A3 — Tag-triggered release workflow.** `.github/workflows/
      release.yml` on `v*` tags: run the gate, then `cargo publish`
      (token from repo secret). Removes the manual local step.
- [ ] **A4 — Verify the crate name is available** on crates.io before
      first publish; if taken, decide on rename vs. ownership.

## B. Private trading surface

The headline functional work. Each exchange already has a public client
+ envelope unwrap to build on; add a signed client alongside it.

- [ ] **B1 — Binance private REST** (`src/binance/private.rs`).
      HMAC-SHA256 over the total query string, `X-MBX-APIKEY` header,
      `timestamp` + `recvWindow` params. Endpoints: account, new/cancel/
      query order, open orders, all orders, my trades; USDT-M futures
      account/position/leverage/order. New `BinanceCredentials`
      (`BINANCE_API_KEY`/`SECRET`, `ZeroizeOnDrop`).
- [x] **B2 — Bybit private REST — done (0.4.0).** `BybitPrivateClient`
      (`src/bybit/private.rs`) ships v5 HMAC-SHA256 `X-BAPI-*` signing + wallet
      balance, place/amend/cancel/cancel-all, open/history orders, positions, set
      leverage, execution list; `BybitCredentials` + `examples/bybit_private_trading.rs`.
- [ ] **B3 — Type Crypto.com private responses.** All 10 methods in
      `src/cryptocom/private.rs` return `serde_json::Value`. Type the
      high-traffic ones first (`get-account-summary`, `create-order`,
      `get-open-orders`, `get-order-detail`, `get-trades`) the way
      Kraken's three got typed in #25 — numeric fields as `String` to
      preserve wire precision.
- [ ] **B4 — Unify order-entry types per venue.** Map the shared
      `Side` / `OrderType` / `TimeInForce` (`src/types.rs`) onto each
      exchange's wire vocabulary so callers don't hand-stringify
      `"GTC"` / `"LIMIT"` per exchange.

## C. Private WebSocket & WS order entry

- [x] **C1 — Kraken private WS** (`executions`, `balances`). `get_websockets_token`
      (`POST /0/private/GetWebSocketsToken` → `KrakenWsToken`) +
      `KrakenConnector::executions_subscription` / `balances_subscription`
      (token-bearing) + parse arms: `executions` → `OrderUpdate` (trades carry
      match_price/size/trade_id), `balances` → `BalanceUpdate` (total only —
      Kraken's v2 channel has no available/hold split, so `hold_balance` = 0).
- [x] **C2 — Binance user-data stream.** `BinanceUserDataRest` listenKey
      lifecycle (`create` / `keepalive` / `close` — API-key header, no HMAC) +
      `BinanceUserDataConnector` streaming `/ws/<listenKey>` (no subscription
      frame). Parses `executionReport` → `OrderUpdate` (fills carry
      match_price/size/trade_id) and `outboundAccountPosition` → `BalanceUpdate`
      (one per asset). First authenticated Binance surface; keepalive *scheduling*
      is the caller's job. Signed account/order REST is still separate (item 2).
- [x] **C3 — Bybit private WS** (`wss://stream.bybit.com/v5/private`) —
      `BybitPrivateConnector`: post-connect `op:"auth"` frame (signed like B2)
      then `order` + `execution` → `OrderUpdate` (executions carry
      match_price/size/trade_id), `position` → `PositionChange`, and `wallet`
      → `BalanceUpdate` (one per coin, account-type in `event`). Driven by the
      additive `ExchangeConnector::auth_message()` hook.
- [x] **C4 — Crypto.com user channel.** `CryptocomUserConnector` over the
      `…/v1/user` URL: signed `public/auth` frame (`auth_message()` hook reusing
      `sign_cryptocom_request`) then `user.order` / `user.trade` → `OrderUpdate`
      (trades carry match_price/size/trade_id) and `user.balance` →
      `BalanceUpdate` (one per `position_balances`). Server heartbeats answered
      via `response_for`. v1 field names with fallbacks for alternate spellings.
- [ ] **C5 — WS order entry beyond KuCoin.** Generalize `WsOrderClient`
      (`src/ws/orders.rs`) — Binance, Bybit, and Kraken all support
      order placement over WS. Extract the clientOid↔oneshot
      correlation core so each exchange supplies only its frame
      builders + ack parser.

## D. Market-data quality

- [ ] **D1 — Local order-book maintainer.** A `LocalOrderBook` that
      applies snapshot → deltas with sequence-number gap detection and
      automatic snapshot resync on gap. Each exchange exposes a
      sequence field (Binance `U`/`u`, Bybit `seq`, Kraken checksum,
      Crypto.com book seq) — surface it on `OrderBookData` and feed the
      maintainer. Add a `checksum` verifier for Kraken/Crypto.com.
- [ ] **D2 — Type `get_exchange_info` / `get_instruments`.** Binance
      `exchangeInfo` and Bybit `instruments-info` return
      `serde_json::Value`. Type the filter shapes (tick size, lot size,
      min notional, contract multiplier) — callers need them for
      order validation / quantity rounding.
- [ ] **D3 — Pagination helpers** for Kraken OHLC/Trades/Spread (the
      `last` cursor) and Bybit cursor-paginated history.
- [ ] **D4 — Optional `rust_decimal` feature.** Keep `String` on the
      wire but offer `*_decimal()` accessors behind a feature for
      callers who want exact arithmetic without `f64` rounding.

## E. Rate limiting

- [ ] **E1 — Proactive REST limiter.** Today only HTTP 429 +
      `Retry-After` is honoured reactively. Add a token-bucket per host
      that reads Binance's `X-MBX-USED-WEIGHT-*` / Kraken's counter /
      Bybit's `X-Bapi-Limit-*` headers and throttles *before* hitting
      the cap.
- [ ] **E2 — Shared WS-send guard for all connectors.** The
      `WsMsgGuard` (100 msg / 10 s) is KuCoin-tuned and KuCoin-only.
      Make the window per-connector-configurable so other venues'
      subscribe/ping cadence is governed too.

## F. Quality, CI & supply chain

- [ ] **F1 — `cargo-deny`** (advisories + licenses + bans) as a CI job
      + `deny.toml`.
- [ ] **F2 — `cargo-semver-checks`** in CI against the last published
      version — catches accidental breaking changes before release.
- [ ] **F3 — Code coverage** (`cargo-llvm-cov` → Codecov/Coveralls)
      with a README badge.
- [ ] **F4 — Property / fuzz tests** for the hand-rolled deserializers
      (Binance kline array, Bybit positional arrays, Kraken tuple
      tickers, Crypto.com single-letter fields) — these are the most
      brittle parsing paths.
- [ ] **F5 — Opt-in live smoke tests** behind a `live-tests` feature +
      env creds, run on a manual/nightly workflow (not PR CI) to catch
      upstream wire-format drift.
- [ ] **F6 — Dependabot / `cargo update` cadence** for the dep tree.

## G. Ergonomics & unified API

- [ ] **G1 — Unified cross-exchange REST traits.** The WS layer is
      already unified through `DataMessage`; REST is not — each client
      has bespoke method names. Add `MarketData` (klines/orderbook/
      ticker/trades) and `Trading` (place/cancel/balances/positions)
      traits so downstream code can be venue-agnostic. Big ergonomic
      win; design carefully to avoid lowest-common-denominator APIs.
- [ ] **G2 — Symbol/instrument normalization.** A helper to translate
      a canonical symbol (`BTC-USDT`) to each venue's format
      (`BTCUSDT`, `XBT/USD`, `BTC_USDT`, …) and back.
- [ ] **G3 — Builder for `WsRunnerConfig` / `SupervisedConfig`** —
      `WsRunnerConfig::builder().max_reconnects(3)…` reads better than
      struct-update syntax at call sites.

## H. New exchanges

Architecture supports it — implement `ExchangeConnector` + REST client +
envelope unwrap. Roughly decreasing demand:

- [ ] **H1 — OKX** — **public WS done (0.5.0)** (`OkxConnector`: trades/tickers/books).
      Remaining: the signed REST/private surface (`{"code":"0","data":[…]}` envelope;
      HMAC-SHA256 with passphrase).
- [ ] **H2 — Coinbase Advanced Trade** — **public WS done (0.5.0)** (`CoinbaseConnector`:
      ticker/trades/level2). Remaining: the signed surface (ECDSA/JWT — a new scheme
      for the crate).
- [ ] **H3 — Hyperliquid** (on-chain perp DEX; EIP-712 signing).
- [ ] **H4 — Gate.io / MEXC / Bitget** as demand warrants.

## I. Docs & community

- [ ] **I1 — Fix "six exchanges" → "five"** in `README.md` (line ~9)
      and any PR/CHANGELOG copy that repeats it. Five are implemented;
      KuCoin's Spot+Futures+UTA is one venue.
- [ ] **I2 — `CONTRIBUTING.md`** documenting the "adding a new
      exchange" checklist (REST client, envelope unwrap fn,
      `ExchangeConnector` impl, the three trait hooks, tests, feature
      flag, example) + issue/PR templates.
- [ ] **I3 — Prometheus/metrics example** wiring the `RunnerEvent` hook
      to counters — the observability hook exists but has no end-to-end
      example.
- [ ] **I4 — Architecture doc / `ARCHITECTURE.md`** explaining the
      runner lifecycle, the connector trait's three extension hooks
      (`subscription_message`, `ping_message`, `response_for`), and the
      supervised-refresh design.

## J. Quick wins / housekeeping

- [ ] **J1 — Reconcile the test count** — the tree is at **311** tests
      (all-features) but `README` says 305 and PR copy says 306. Update
      the README, or drop the hard-coded count so it can't drift again.
- [ ] **J2 — Resolve the `@ticker` TODO** in `src/binance/ws.rs:365`
      (last-price source not wired) — either wire it or document the
      `bookTicker` limitation inline.
- [ ] **J3 — `cargo doc` landing polish** — crate-level example +
      module overview already good; add per-exchange module-doc
      examples once private surfaces land.
- [ ] **J4 — MSRV recheck** when new clippy lints land
      (`duration_suboptimal_units` is currently allowed only because
      `from_mins`/`from_hours` postdate MSRV 1.94.1; revisit on bump).

---

## Done — original roadmap (complete)

The full build-out (PRs #1–#27) is finished. Highlights, newest first;
see `CHANGELOG.md` for per-version detail and git history for the
verbose design notes that used to live here.

- **CI + clean baseline** (#27) — Actions workflow + 24→0 warnings.
- **`prelude`** (#26) — one-line glob import.
- **Kraken typed `Value` endpoints** (#25) — trades-history, ledger,
  withdrawal-status.
- **`release.sh` publish-safe + package `exclude`** (#24).
- **Per-exchange Cargo features** (#23).
- **Runnable examples per exchange** (#22).
- **CHANGELOG + README rewrite** (#20–#21).
- **Crypto.com** public REST / private REST / public WS (#16–#19) —
  fourth envelope, third signing scheme, server-initiated heartbeat via
  the new `response_for` connector hook.
- **Kraken** public REST / private REST (HMAC-SHA512) / public WS v2
  (#13–#15).
- **KuCoin WS order placement** (`WsOrderClient`) + spot-margin + UTA
  REST (#11–#13).
- **Bybit** public REST + WS, **Binance** public REST + WS (#7–#11) —
  `PublicRestClient`, `DataMessage::Candle`/`FundingRate`, the
  `ping_message` connector hook.
- **Disconnection hardening** (#1–#6) — `run_feed_supervised`,
  connect/idle timeouts, cascade-start WARN, `RunnerEvent` hook, tighter
  defaults. (Root cause: stale WS tokens caused 9-min reconnect-exhaust
  blackouts; supervised refresh cuts that to ~10 s.)

### Architecture extension points (for new exchanges)

- `ExchangeConnector` trait (`src/actors.rs`) — implement `parse_message`
  + the three optional hooks: `subscription_message`, `ping_message`,
  `response_for` (server-initiated heartbeat reply).
- `PublicRestClient` (`src/http.rs`) — unauthenticated HTTP with jittered
  backoff + 429/`Retry-After`; wrap it and add a signing layer for
  private endpoints.
- One free `unwrap_<exchange>_envelope<T>` fn per module — surfaces
  non-OK envelopes as `ExchangeError::Api`.
- Unified `DataMessage` (`src/actors.rs`) — the cross-exchange feed type
  every connector normalizes into.
