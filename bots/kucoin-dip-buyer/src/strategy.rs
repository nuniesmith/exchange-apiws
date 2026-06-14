//! Pure "buy the dip" strategy logic — no I/O, fully unit-testable.
//!
//! The strategy is a tiny three-state machine driven by two inputs:
//!
//! - **closed candles** → RSI + trend-EMA decide *when to enter* a long.
//! - **the live price**  → take-profit / stop-loss / trailing-stop decide
//!   *when (and how much) to exit*.
//!
//! Nothing here talks to the network. The orchestrator in `main.rs` feeds it
//! indicator series and prices, and turns the returned [`Action`]s into real
//! KuCoin orders (or dry-run log lines). That split keeps every trading rule
//! deterministic and covered by the unit tests at the bottom of this file.

/// Tunable strategy parameters. Constructed from environment variables in
/// `main.rs`; defaults live in [`Config::default`].
#[derive(Clone, Debug)]
pub struct Config {
    /// Futures contract symbol, e.g. `"XBTUSDTM"` (BTC/USDT perpetual).
    pub symbol: String,
    /// Kline timeframe in minutes, KuCoin-style (`"1"`, `"5"`, `"15"`, `"60"`, …).
    pub granularity: String,
    /// How many historical bars to pull for the indicators.
    pub lookback: usize,

    /// RSI period (classic dip indicator). 14 is the textbook default.
    pub rsi_period: usize,
    /// RSI level that marks "oversold". We enter on a cross *up* through it.
    pub rsi_oversold: f64,

    /// Trend filter EMA period. Only buy dips while price is above this EMA.
    pub trend_ema_period: usize,
    /// Disable to buy every oversold cross regardless of the broader trend.
    pub use_trend_filter: bool,

    /// Leverage requested per order. With a ~$38 account this has to be ≥ ~2–3
    /// just to afford KuCoin's 1-contract minimum on BTC — see the README.
    pub leverage: u32,
    /// Fraction of available balance to commit as margin on an entry.
    pub risk_fraction: f64,
    /// Hard cap on contracts per position.
    pub max_contracts: u32,

    /// Take-profit trigger, as a fraction above average entry (0.02 = +2 %).
    pub take_profit_pct: f64,
    /// Stop-loss trigger, as a fraction below average entry (0.03 = −3 %).
    pub stop_loss_pct: f64,
    /// At take-profit, sell this fraction of the position and let the rest run
    /// (0.90 = "sell 90 %, keep 10 %"). Ignored when the position is too small
    /// to split (e.g. a single contract) — then the whole lot trails instead.
    pub sell_fraction: f64,
    /// Trailing-stop distance for the kept "runner", as a fraction below its
    /// high-water mark (0.015 = 1.5 %). Floored at break-even.
    pub trail_pct: f64,

    /// Seconds between kline polls / indicator refreshes.
    pub poll_secs: u64,
    /// `false` (default) = dry-run: decisions are logged, no orders are sent.
    pub live: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            symbol: "XBTUSDTM".to_string(),
            granularity: "15".to_string(),
            lookback: 400,
            rsi_period: 14,
            rsi_oversold: 30.0,
            trend_ema_period: 200,
            use_trend_filter: true,
            leverage: 3,
            risk_fraction: 0.90,
            max_contracts: 50,
            take_profit_pct: 0.02,
            stop_loss_pct: 0.03,
            sell_fraction: 0.90,
            trail_pct: 0.015,
            poll_secs: 30,
            live: false,
        }
    }
}

/// Where the bot is in its lifecycle for the (single) tracked position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No position — waiting for an oversold-reversal entry signal.
    Flat,
    /// Holding a freshly opened long — watching for take-profit / stop-loss.
    Long,
    /// Holding the kept remainder after a take-profit — trailing it for more
    /// upside, exiting on a pullback (never below break-even).
    Runner,
}

/// The bot's view of its open long. `contracts` is always ≥ 0 here (the bot
/// only goes long); 0 means flat.
#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    /// Open size in contracts.
    pub contracts: i64,
    /// Volume-weighted average entry price.
    pub entry: f64,
    /// Highest price seen since entry — drives the trailing stop.
    pub high_since_entry: f64,
}

/// An exit instruction the orchestrator should carry out. Entries are handled
/// separately via [`Strategy::entry_signal`] because they require async
/// position sizing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Do nothing this tick.
    Hold,
    /// Reduce the position by `contracts` (a reduce-only sell) — the scale-out.
    Reduce {
        /// Contracts to sell now.
        contracts: i64,
        /// Human-readable reason for the log.
        reason: &'static str,
    },
    /// Close the entire remaining position at market.
    Close {
        /// Human-readable reason for the log.
        reason: &'static str,
    },
    /// Take-profit hit but the position can't be split (too small) — flip the
    /// whole lot into the trailing "runner" phase without sending any order.
    StartRunner,
}

/// The dip-buying state machine. Owns its [`Config`], [`Phase`] and
/// [`Position`]; decisions are pure functions of that state plus the latest
/// inputs, and state only advances through the `confirm_*` methods once an
/// order has actually been placed (or simulated, in dry-run).
pub struct Strategy {
    cfg: Config,
    phase: Phase,
    pos: Position,
}

impl Strategy {
    /// Create a flat strategy from the given config.
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            phase: Phase::Flat,
            pos: Position::default(),
        }
    }

    /// Current lifecycle phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Current tracked position.
    pub fn position(&self) -> Position {
        self.pos
    }

    /// Read-only access to the config.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    // ── Entry ──────────────────────────────────────────────────────────────

    /// Decide whether the latest **closed** bar is a dip-buy entry.
    ///
    /// `closes`, `rsi` and `ema` are parallel series over the same closed bars
    /// (same length; leading warm-up values may be `NaN`). The signal fires on
    /// a *cross up* through the oversold level — RSI was at/below it on the
    /// prior bar and is above it now — which waits for momentum to turn rather
    /// than catching a falling knife. With the trend filter on, the close must
    /// also be above the trend EMA ("only buy dips in an uptrend").
    ///
    /// Only meaningful while [`Phase::Flat`]; returns `false` otherwise.
    pub fn entry_signal(&self, closes: &[f64], rsi: &[f64], ema: &[f64]) -> bool {
        if self.phase != Phase::Flat {
            return false;
        }
        let n = closes.len();
        if n < 2 || rsi.len() != n || ema.len() != n {
            return false;
        }
        let i = n - 1;
        let (r_prev, r_now) = (rsi[i - 1], rsi[i]);
        if !r_prev.is_finite() || !r_now.is_finite() {
            return false;
        }
        let crossed_up = r_prev <= self.cfg.rsi_oversold && r_now > self.cfg.rsi_oversold;
        if !crossed_up {
            return false;
        }
        if self.cfg.use_trend_filter {
            let e = ema[i];
            if !e.is_finite() || closes[i] <= e {
                return false;
            }
        }
        true
    }

    // ── Exit ───────────────────────────────────────────────────────────────

    /// Update the trailing high-water mark with the latest price. Call once per
    /// price tick *before* [`Strategy::decide`].
    pub fn observe_price(&mut self, price: f64) {
        if self.phase != Phase::Flat && price > self.pos.high_since_entry {
            self.pos.high_since_entry = price;
        }
    }

    /// Decide the exit action for the current price and state.
    ///
    /// - **Long**: stop-loss below entry, or take-profit above it. On
    ///   take-profit we scale out `sell_fraction` of the position; if that
    ///   rounds to zero contracts (a tiny position) we instead start trailing
    ///   the whole lot via [`Action::StartRunner`].
    /// - **Runner**: exit on a trailing stop `trail_pct` below the high, but
    ///   never below break-even (the kept remainder is risk-free).
    pub fn decide(&self, price: f64) -> Action {
        match self.phase {
            Phase::Flat => Action::Hold,
            Phase::Long => {
                let e = self.pos.entry;
                if price <= e * (1.0 - self.cfg.stop_loss_pct) {
                    return Action::Close {
                        reason: "stop-loss",
                    };
                }
                if price >= e * (1.0 + self.cfg.take_profit_pct) {
                    let sell = (self.pos.contracts as f64 * self.cfg.sell_fraction).floor() as i64;
                    if sell >= 1 && sell < self.pos.contracts {
                        return Action::Reduce {
                            contracts: sell,
                            reason: "take-profit (scale-out)",
                        };
                    }
                    // Can't keep a fraction of a single contract — trail it all.
                    return Action::StartRunner;
                }
                Action::Hold
            }
            Phase::Runner => {
                let breakeven = self.pos.entry;
                let trail_stop = (self.pos.high_since_entry * (1.0 - self.cfg.trail_pct)).max(breakeven);
                if price <= trail_stop {
                    return Action::Close {
                        reason: "trailing-stop",
                    };
                }
                Action::Hold
            }
        }
    }

    // ── State transitions (call only after the order succeeds) ─────────────

    /// Record a completed entry fill: now [`Phase::Long`].
    pub fn confirm_entry(&mut self, contracts: i64, entry: f64) {
        self.pos = Position {
            contracts,
            entry,
            high_since_entry: entry,
        };
        self.phase = Phase::Long;
    }

    /// Record a completed scale-out of `sold` contracts: now [`Phase::Runner`]
    /// on the remainder.
    pub fn confirm_reduce(&mut self, sold: i64) {
        self.pos.contracts = (self.pos.contracts - sold).max(0);
        self.phase = Phase::Runner;
    }

    /// Flip the whole (unsplittable) position into the trailing runner without
    /// changing its size.
    pub fn confirm_runner(&mut self) {
        self.phase = Phase::Runner;
    }

    /// Record a full close: back to [`Phase::Flat`].
    pub fn confirm_close(&mut self) {
        self.pos = Position::default();
        self.phase = Phase::Flat;
    }

    /// Reconcile the tracked position with the exchange's authoritative view
    /// (used in live mode each poll). Preserves the local phase/high but
    /// corrects size and entry, and snaps to flat if the exchange shows none.
    pub fn reconcile(&mut self, contracts: i64, entry: f64) {
        if contracts <= 0 {
            if self.phase != Phase::Flat {
                self.confirm_close();
            }
            return;
        }
        self.pos.contracts = contracts;
        if entry.is_finite() && entry > 0.0 {
            self.pos.entry = entry;
            if self.pos.high_since_entry < entry {
                self.pos.high_since_entry = entry;
            }
        }
        if self.phase == Phase::Flat {
            self.phase = Phase::Long;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn entry_fires_on_oversold_cross_up_in_uptrend() {
        let s = Strategy::new(cfg());
        // RSI crosses 28 -> 35 (up through 30), close above the trend EMA.
        let closes = vec![100.0, 101.0];
        let rsi = vec![28.0, 35.0];
        let ema = vec![99.0, 99.5];
        assert!(s.entry_signal(&closes, &rsi, &ema));
    }

    #[test]
    fn no_entry_when_still_below_threshold() {
        let s = Strategy::new(cfg());
        let closes = vec![100.0, 101.0];
        let rsi = vec![25.0, 28.0]; // never crosses above 30
        let ema = vec![99.0, 99.5];
        assert!(!s.entry_signal(&closes, &rsi, &ema));
    }

    #[test]
    fn no_entry_without_a_fresh_cross() {
        let s = Strategy::new(cfg());
        let closes = vec![100.0, 101.0];
        let rsi = vec![35.0, 40.0]; // already above 30 on the prior bar
        let ema = vec![99.0, 99.5];
        assert!(!s.entry_signal(&closes, &rsi, &ema));
    }

    #[test]
    fn trend_filter_blocks_dip_below_ema() {
        let s = Strategy::new(cfg());
        let closes = vec![100.0, 98.0]; // below the trend EMA
        let rsi = vec![28.0, 35.0];
        let ema = vec![99.0, 99.5];
        assert!(!s.entry_signal(&closes, &rsi, &ema));
    }

    #[test]
    fn trend_filter_off_allows_downtrend_dip() {
        let mut c = cfg();
        c.use_trend_filter = false;
        let s = Strategy::new(c);
        let closes = vec![100.0, 98.0];
        let rsi = vec![28.0, 35.0];
        let ema = vec![99.0, 99.5];
        assert!(s.entry_signal(&closes, &rsi, &ema));
    }

    #[test]
    fn long_stop_loss_closes_all() {
        let mut s = Strategy::new(cfg()); // stop-loss 3 %
        s.confirm_entry(1, 100.0);
        assert_eq!(s.decide(96.9), Action::Close { reason: "stop-loss" });
    }

    #[test]
    fn long_take_profit_scales_out_ninety_percent() {
        let mut s = Strategy::new(cfg()); // tp 2 %, sell_fraction 0.9
        s.confirm_entry(10, 100.0);
        // +2 % -> sell floor(10 * 0.9) = 9, keep 1 as the runner.
        assert_eq!(
            s.decide(102.0),
            Action::Reduce {
                contracts: 9,
                reason: "take-profit (scale-out)"
            }
        );
    }

    #[test]
    fn single_contract_take_profit_starts_runner_instead_of_selling() {
        let mut s = Strategy::new(cfg());
        s.confirm_entry(1, 100.0); // floor(1 * 0.9) = 0 -> can't split
        assert_eq!(s.decide(102.0), Action::StartRunner);
    }

    #[test]
    fn runner_trails_and_exits_on_pullback_above_breakeven() {
        let mut s = Strategy::new(cfg()); // trail 1.5 %
        s.confirm_entry(1, 100.0);
        s.confirm_runner();
        s.observe_price(110.0); // high-water mark = 110
        assert_eq!(s.decide(109.0), Action::Hold); // 109 > 110*0.985 = 108.35
        assert_eq!(
            s.decide(108.0),
            Action::Close {
                reason: "trailing-stop"
            }
        ); // 108 < 108.35
    }

    #[test]
    fn runner_stop_never_drops_below_breakeven() {
        let mut s = Strategy::new(cfg());
        s.confirm_entry(1, 100.0);
        s.confirm_runner();
        s.observe_price(100.5); // barely above entry; 100.5*0.985 < 100
        // Trailing stop is floored at break-even (100), so a dip to 99.9 exits.
        assert_eq!(
            s.decide(99.9),
            Action::Close {
                reason: "trailing-stop"
            }
        );
    }

    #[test]
    fn full_lifecycle_transitions() {
        let mut s = Strategy::new(cfg());
        assert_eq!(s.phase(), Phase::Flat);
        s.confirm_entry(10, 100.0);
        assert_eq!(s.phase(), Phase::Long);
        s.confirm_reduce(9);
        assert_eq!(s.phase(), Phase::Runner);
        assert_eq!(s.position().contracts, 1);
        s.confirm_close();
        assert_eq!(s.phase(), Phase::Flat);
        assert_eq!(s.position().contracts, 0);
    }

    #[test]
    fn reconcile_snaps_to_flat_when_exchange_shows_none() {
        let mut s = Strategy::new(cfg());
        s.confirm_entry(5, 100.0);
        s.reconcile(0, 0.0);
        assert_eq!(s.phase(), Phase::Flat);
    }
}
