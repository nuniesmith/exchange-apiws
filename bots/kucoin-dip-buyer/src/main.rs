//! KuCoin BTC-futures "buy the dip" bot.
//!
//! Ties the two crates together:
//! - **`indicators-ta`** computes RSI + a trend EMA from closed candles.
//! - **`exchange-apiws`** pulls live data (REST klines + a public WebSocket
//!   ticker feed) and places the orders.
//!
//! The trading rules live in [`strategy`] (pure, unit-tested); this file is
//! just the I/O wiring: fetch data → ask the strategy → execute.
//!
//! ## Safety
//! Runs in **dry-run** mode by default — it logs the orders it *would* place
//! but sends nothing. Set `DIP_LIVE=1` (and provide API keys) to trade for
//! real. See the README for the full config and the risk notes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::ws::{SupervisedConfig, WsFeedEndpoint, run_feed_supervised};
use exchange_apiws::{Candle, Credentials, KuCoin, KucoinConnector, OrderType, Side};

use indicators::{ema as ema_series, rsi as rsi_series};

mod strategy;
use strategy::{Action, Config, Phase, Strategy};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = load_config();

    // Credentials: required for live trading; optional for a dry-run (public
    // market data and the public WS feed don't need a valid signature).
    let (creds, have_creds) = match Credentials::from_env() {
        Ok(c) => (c, true),
        Err(_) => {
            if cfg.live {
                bail!(
                    "DIP_LIVE=1 but KC_KEY / KC_SECRET / KC_PASSPHRASE are not set — \
                     refusing to trade live without API keys."
                );
            }
            warn!("no API keys found — running in dry-run with public data only");
            (Credentials::new("", "", ""), false)
        }
    };

    let kucoin = KuCoin::futures(creds);
    let env = kucoin.env();
    let client = Arc::new(kucoin.rest_client()?);

    // Contract metadata gives us the multiplier (BTC per contract) we need for
    // position sizing and affordability checks.
    let contract = client
        .get_contract(&cfg.symbol)
        .await
        .with_context(|| format!("fetching contract metadata for {}", cfg.symbol))?;
    let mult = contract
        .multiplier
        .context("contract returned no multiplier — cannot size positions")?;

    // Balance: live always reads the real account; dry-run reads it if keys are
    // present, otherwise falls back to a paper balance (default 38 USDT).
    let paper_balance: f64 = std::env::var("DIP_PAPER_BALANCE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(38.0);
    let mut balance = if cfg.live {
        client.get_balance("USDT").await.context("reading USDT balance")?
    } else if have_creds {
        client.get_balance("USDT").await.unwrap_or_else(|e| {
            warn!(error = %e, "get_balance failed — using paper balance");
            paper_balance
        })
    } else {
        paper_balance
    };

    // A reference price for the startup banner.
    let price0 = client
        .fetch_klines(&cfg.symbol, 2, &cfg.granularity)
        .await
        .context("fetching an initial price")?
        .last()
        .map(|c| c.close)
        .unwrap_or(0.0);

    // Maintenance-margin rate sets how far below entry liquidation sits; fall
    // back to a typical 0.5 % if the contract doesn't report one.
    let maint_margin = contract.maint_margin_rate.unwrap_or(0.005);
    print_banner(&cfg, mult, balance, price0, maint_margin);

    // ── Public WebSocket ticker feed (live price) ──────────────────────────
    // Supervised runner: it re-negotiates the token and resubscribes whenever a
    // reconnect cascade exhausts — the recommended pattern for long-running bots.
    let token = client.get_ws_token_public().await.context("negotiating WS token")?;
    let connector = Arc::new(KucoinConnector::new(&token, env)?);

    let refresh = {
        let client = client.clone();
        let symbol = cfg.symbol.clone();
        move || {
            let client = client.clone();
            let symbol = symbol.clone();
            async move {
                let token = client.get_ws_token_public().await?;
                let conn = KucoinConnector::new(&token, env)?;
                let mut subs = vec![];
                if let Some(s) = conn.ticker_subscription(&symbol) {
                    subs.push(s);
                }
                Ok(WsFeedEndpoint {
                    url: conn.ws_url().to_string(),
                    subscriptions: subs,
                })
            }
        }
    };

    let (tx, mut rx) = mpsc::channel::<DataMessage>(2048);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let feed = tokio::spawn(run_feed_supervised(
        connector,
        tx,
        SupervisedConfig::default(),
        shutdown_rx,
        refresh,
    ));

    // ── Strategy + main loop ───────────────────────────────────────────────
    let mut strat = Strategy::new(cfg.clone());
    let mut last_price = 0.0_f64;

    // Prime indicators / position once before the loop.
    if let Err(e) = evaluate_bar(&client, &mut strat, mult, &mut balance, &mut last_price).await {
        warn!(error = %e, "initial bar evaluation failed (will retry on the next poll)");
    }

    let mut poll = tokio::time::interval(Duration::from_secs(cfg.poll_secs.max(1)));
    poll.tick().await; // consume the immediate first tick

    info!(poll_secs = cfg.poll_secs, "entering main loop — Ctrl-C to stop");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("shutdown requested — leaving any open position untouched");
                let _ = shutdown_tx.send(true);
                break;
            }
            // Periodic indicator refresh + entry evaluation.
            _ = poll.tick() => {
                if let Err(e) = evaluate_bar(&client, &mut strat, mult, &mut balance, &mut last_price).await {
                    warn!(error = %e, "bar evaluation failed");
                }
            }
            // Live price ticks → fast take-profit / stop-loss reaction.
            maybe = rx.recv() => match maybe {
                Some(DataMessage::Ticker(t)) => {
                    last_price = t.price;
                    if strat.phase() != Phase::Flat
                        && let Err(e) = react_exit(&client, &mut strat, last_price).await
                    {
                        warn!(error = %e, "exit check failed");
                    }
                }
                Some(_) => {} // trades / books / etc. — ignored
                None => { warn!("feed channel closed — exiting"); break; }
            }
        }
    }

    let _ = feed.await;
    info!("stopped");
    Ok(())
}

/// Refresh indicators from closed candles and, if flat, look for an entry.
/// In live mode it first reconciles the tracked position and balance with the
/// exchange so a restart (or a manual fill) can't desync the bot.
async fn evaluate_bar(
    client: &exchange_apiws::KuCoinClient,
    strat: &mut Strategy,
    mult: f64,
    balance: &mut f64,
    last_price: &mut f64,
) -> Result<()> {
    let (symbol, gran, lookback, rsi_p, ema_p, live) = {
        let c = strat.config();
        (
            c.symbol.clone(),
            c.granularity.clone(),
            c.lookback,
            c.rsi_period,
            c.trend_ema_period,
            c.live,
        )
    };

    if live {
        match client.get_position(&symbol).await {
            Ok(p) => strat.reconcile(p.current_qty as i64, p.avg_entry_price.unwrap_or(0.0)),
            Err(e) => warn!(error = %e, "get_position failed — keeping local state"),
        }
        match client.get_balance("USDT").await {
            Ok(b) => *balance = b,
            Err(e) => warn!(error = %e, "get_balance failed — keeping last balance"),
        }
    }

    let candles = client
        .fetch_klines_extended(&symbol, lookback, &gran, 200)
        .await
        .context("fetching klines")?;
    let closes = closed_closes(candles, &gran);

    let needed = rsi_p.max(ema_p) + 2;
    if closes.len() < needed {
        warn!(have = closes.len(), needed, "not enough closed bars yet — waiting");
        return Ok(());
    }

    let rsi = rsi_series(&closes, rsi_p).context("computing RSI")?;
    let ema = ema_series(&closes, ema_p).context("computing trend EMA")?;
    let i = closes.len() - 1;

    if *last_price <= 0.0 {
        *last_price = closes[i];
    }
    let price = *last_price;

    info!(
        phase = ?strat.phase(),
        rsi = format!("{:.1}", rsi[i]),
        trend_ema = format!("{:.1}", ema[i]),
        close = format!("{:.1}", closes[i]),
        price = format!("{:.1}", price),
        "bar"
    );

    if strat.phase() == Phase::Flat {
        if strat.entry_signal(&closes, &rsi, &ema) {
            info!("ENTRY signal — RSI crossed up through oversold in an uptrend");
            try_enter(client, strat, mult, *balance, price).await?;
        }
    } else {
        // Also re-check exits on the bar close, not just on ticker ticks.
        react_exit(client, strat, price).await?;
    }
    Ok(())
}

/// Size and open a long, honouring the 1-contract minimum and the account's
/// affordable margin. Skips the entry (with a clear warning) if even one
/// contract can't be afforded at the configured leverage.
async fn try_enter(
    client: &exchange_apiws::KuCoinClient,
    strat: &mut Strategy,
    mult: f64,
    balance: f64,
    price: f64,
) -> Result<()> {
    let (symbol, lev, live, rf, maxc) = {
        let c = strat.config();
        (c.symbol.clone(), c.leverage, c.live, c.risk_fraction, c.max_contracts)
    };

    // Desired size: the exchange's own sizing in live mode, the same formula
    // locally in dry-run (so it works with no keys).
    let desired = if live {
        client
            .calc_contracts(&symbol, price, balance, lev, rf, maxc)
            .await
            .context("calc_contracts")? as i64
    } else {
        let margin_per_ct = price * mult / f64::from(lev);
        (((balance * rf) / margin_per_ct).floor() as i64).clamp(1, i64::from(maxc))
    };

    // Trim to what the balance can actually post as margin (98 % leaves a fee
    // buffer). With ~38 USDT this is usually exactly 1 contract.
    let margin_per_ct = price * mult / f64::from(lev);
    let mut n = desired;
    while n >= 1 && (n as f64) * margin_per_ct > balance * 0.98 {
        n -= 1;
    }
    if n < 1 {
        warn!(
            need_margin = format!("{margin_per_ct:.2}"),
            balance = format!("{balance:.2}"),
            leverage = lev,
            "insufficient balance for the 1-contract minimum — raise DIP_LEVERAGE or deposit more; skipping entry"
        );
        return Ok(());
    }

    let notional = n as f64 * price * mult;
    let margin = n as f64 * margin_per_ct;
    if live {
        client
            .place_order(&symbol, Side::Buy, n as u32, lev, OrderType::Market, None, None, false, None)
            .await
            .context("placing entry order")?;
        info!(
            contracts = n, leverage = lev,
            notional = format!("{notional:.2}"), margin = format!("{margin:.2}"),
            price = format!("{price:.1}"),
            "LIVE: opened long (market buy)"
        );
    } else {
        info!(
            contracts = n, leverage = lev,
            notional = format!("{notional:.2}"), margin = format!("{margin:.2}"),
            price = format!("{price:.1}"),
            "DRY-RUN: would open long (market buy)"
        );
    }
    strat.confirm_entry(n, price);
    Ok(())
}

/// Run the exit logic for the current price: scale-out, full close, or flip a
/// too-small position into the trailing runner.
async fn react_exit(
    client: &exchange_apiws::KuCoinClient,
    strat: &mut Strategy,
    price: f64,
) -> Result<()> {
    strat.observe_price(price);
    let action = strat.decide(price);
    let (symbol, lev, live) = {
        let c = strat.config();
        (c.symbol.clone(), c.leverage, c.live)
    };

    match action {
        Action::Hold => {}
        Action::StartRunner => {
            strat.confirm_runner();
            info!("take-profit reached but the position is too small to split — trailing the whole lot");
        }
        Action::Reduce { contracts, reason } => {
            if live {
                // reduce_only = true: this sell can only shrink the long.
                client
                    .place_order(&symbol, Side::Sell, contracts as u32, lev, OrderType::Market, None, None, true, None)
                    .await
                    .context("placing scale-out order")?;
            }
            info!(contracts, reason, live, "SCALE-OUT: sell (reduce-only) — keeping the remainder as a runner");
            strat.confirm_reduce(contracts);
        }
        Action::Close { reason } => {
            let qty = strat.position().contracts;
            if live && qty > 0 {
                client
                    .close_position(&symbol, qty as i32, lev)
                    .await
                    .context("closing position")?;
            }
            info!(contracts = qty, reason, live, "CLOSE: flat again");
            strat.confirm_close();
        }
    }
    Ok(())
}

/// Drop any trailing in-progress candle and return the closing prices of the
/// finished bars, oldest-first.
fn closed_closes(mut candles: Vec<Candle>, gran: &str) -> Vec<f64> {
    let gran_min: i64 = gran.parse().unwrap_or(15);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(i64::MAX);
    candles.sort_by_key(|c| c.time);
    while let Some(last) = candles.last() {
        if last.time + gran_min * 60_000 > now_ms {
            candles.pop();
        } else {
            break;
        }
    }
    candles.iter().map(|c| c.close).collect()
}

/// Print a one-screen summary of the run, with the affordability + risk notes
/// that matter most for a tiny account.
fn print_banner(cfg: &Config, mult: f64, balance: f64, price0: f64, maint_margin: f64) {
    let margin_one = price0 * mult / f64::from(cfg.leverage);
    let affordable = if margin_one > 0.0 {
        ((balance * 0.98) / margin_one).floor() as i64
    } else {
        0
    };
    // Approx isolated-long liquidation distance below entry: 1/leverage minus
    // the maintenance-margin rate (ignores fees, so the real liquidation is a
    // touch closer). This is what the stop-loss has to stay inside of.
    let liq_dist = (1.0 / f64::from(cfg.leverage) - maint_margin).max(0.0);
    let liq_price = price0 * (1.0 - liq_dist);

    eprintln!("──────────────────────────────────────────────────────────────");
    eprintln!(" KuCoin dip-buyer — {}", if cfg.live { "*** LIVE TRADING ***" } else { "DRY-RUN (no orders sent)" });
    eprintln!("──────────────────────────────────────────────────────────────");
    eprintln!(" symbol            {}  (mult {mult} BTC/contract)", cfg.symbol);
    eprintln!(" timeframe         {} min, {} bars lookback", cfg.granularity, cfg.lookback);
    eprintln!(" entry             RSI({}) cross up > {}  +  trend filter {}",
        cfg.rsi_period, cfg.rsi_oversold,
        if cfg.use_trend_filter { format!("close > EMA({})", cfg.trend_ema_period) } else { "off".into() });
    eprintln!(" exit              TP +{:.1}% / SL -{:.1}% / sell {:.0}% & trail kept {:.1}%",
        cfg.take_profit_pct * 100.0, cfg.stop_loss_pct * 100.0,
        cfg.sell_fraction * 100.0, cfg.trail_pct * 100.0);
    eprintln!(" sizing            {}x leverage, risk {:.0}% of balance, cap {} contracts",
        cfg.leverage, cfg.risk_fraction * 100.0, cfg.max_contracts);
    eprintln!(" balance           {balance:.2} USDT   (price ~{price0:.0})");
    eprintln!(" 1 contract        ~{:.2} USDT notional, ~{margin_one:.2} USDT margin at {}x",
        price0 * mult, cfg.leverage);
    eprintln!(" liquidation       long liquidates ~{liq_price:.0}  (~{:.1}% below entry, isolated)",
        liq_dist * 100.0);
    eprintln!("──────────────────────────────────────────────────────────────");

    if affordable < 1 {
        eprintln!(" ⚠  Balance can't post margin for even 1 contract at {}x.", cfg.leverage);
        eprintln!("    Raise DIP_LEVERAGE or deposit more — no entries will fire.");
    } else {
        eprintln!(" ▸ Affordable size now: ~{affordable} contract(s).");
        if affordable < 10 {
            eprintln!("   With <10 contracts the 'sell 90% / keep 10%' split can't trigger;");
            eprintln!("   the bot trails the whole position at take-profit instead.");
        }
    }
    // The stop-loss only protects you if it triggers *before* liquidation.
    if liq_dist > 0.0 {
        if cfg.stop_loss_pct >= liq_dist {
            eprintln!(" ⚠  STOP-LOSS (-{:.1}%) is AT/BEYOND liquidation (~-{:.1}%) at {}x —",
                cfg.stop_loss_pct * 100.0, liq_dist * 100.0, cfg.leverage);
            eprintln!("    you'd be liquidated before the stop fills. Lower DIP_LEVERAGE or DIP_STOP_LOSS_PCT.");
        } else if cfg.stop_loss_pct > 0.7 * liq_dist {
            eprintln!(" ⚠  STOP-LOSS (-{:.1}%) is close to liquidation (~-{:.1}%) at {}x —",
                cfg.stop_loss_pct * 100.0, liq_dist * 100.0, cfg.leverage);
            eprintln!("    a fast wick could liquidate first. Leave more room (lower leverage / tighter stop).");
        }
    }
    if cfg.leverage >= 10 {
        eprintln!(" ⚠  {}x is very high leverage — moves are amplified; size down if unsure.", cfg.leverage);
    }
    if cfg.live {
        eprintln!(" ⚠  LIVE mode: real orders on real money. Ctrl-C stops the bot but");
        eprintln!("    does NOT close an open position.");
    }
    eprintln!("──────────────────────────────────────────────────────────────");
}

// ── Config from environment ────────────────────────────────────────────────

fn env_str(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.is_empty())
}
fn env_num<T: std::str::FromStr>(k: &str) -> Option<T> {
    env_str(k).and_then(|s| s.parse().ok())
}
fn env_bool(k: &str) -> Option<bool> {
    env_str(k).map(|s| matches!(s.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
}

/// Build the [`Config`] from `DIP_*` environment variables, falling back to
/// [`Config::default`] for anything unset.
fn load_config() -> Config {
    let mut c = Config::default();
    if let Some(v) = env_str("DIP_SYMBOL") { c.symbol = v; }
    if let Some(v) = env_str("DIP_GRANULARITY") { c.granularity = v; }
    if let Some(v) = env_num("DIP_LOOKBACK") { c.lookback = v; }
    if let Some(v) = env_num("DIP_RSI_PERIOD") { c.rsi_period = v; }
    if let Some(v) = env_num("DIP_RSI_OVERSOLD") { c.rsi_oversold = v; }
    if let Some(v) = env_num("DIP_TREND_EMA_PERIOD") { c.trend_ema_period = v; }
    if let Some(v) = env_bool("DIP_TREND_FILTER") { c.use_trend_filter = v; }
    if let Some(v) = env_num("DIP_LEVERAGE") { c.leverage = v; }
    if let Some(v) = env_num("DIP_RISK_FRACTION") { c.risk_fraction = v; }
    if let Some(v) = env_num("DIP_MAX_CONTRACTS") { c.max_contracts = v; }
    if let Some(v) = env_num("DIP_TAKE_PROFIT_PCT") { c.take_profit_pct = v; }
    if let Some(v) = env_num("DIP_STOP_LOSS_PCT") { c.stop_loss_pct = v; }
    if let Some(v) = env_num("DIP_SELL_FRACTION") { c.sell_fraction = v; }
    if let Some(v) = env_num("DIP_TRAIL_PCT") { c.trail_pct = v; }
    if let Some(v) = env_num("DIP_POLL_SECS") { c.poll_secs = v; }
    if let Some(v) = env_bool("DIP_LIVE") { c.live = v; }
    c
}
