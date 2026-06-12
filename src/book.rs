//! Local order-book maintenance — assembles snapshot + delta streams into a
//! synchronized book with sequence-gap detection.
//!
//! Connectors emit [`OrderBookData`] messages (snapshots and incremental
//! deltas), but applying deltas blindly silently corrupts the book the
//! moment a message is dropped. [`LocalOrderBook`] applies each message,
//! tracks the venue's update IDs ([`OrderBookData::first_update_id`] /
//! [`OrderBookData::last_update_id`]), and reports a [`BookApply::Gap`]
//! when the stream skips ahead — at which point the book marks itself
//! unsynced and ignores further deltas until the caller re-seeds it with a
//! fresh snapshot (REST or the stream's next snapshot frame).
//!
//! Use one `LocalOrderBook` per (exchange, symbol) stream.
//!
//! # Example
//!
//! ```
//! use exchange_apiws::actors::{DataMessage, OrderBookData};
//! use exchange_apiws::book::{BookApply, LocalOrderBook};
//!
//! fn on_message(book: &mut LocalOrderBook, msg: DataMessage) {
//!     if let DataMessage::OrderBook(ob) = msg {
//!         match book.apply(&ob) {
//!             BookApply::Gap { expected, got } => {
//!                 // Sequence jumped — refetch a snapshot before trusting
//!                 // the book again.
//!                 eprintln!("book gap: expected {expected}, got {got}");
//!             }
//!             _ => {
//!                 if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) {
//!                     println!("top of book: {} / {}", bid[0], ask[0]);
//!                 }
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # Venue coverage
//!
//! Gap detection requires the venue to stamp sequence IDs on book messages:
//!
//! | Venue | IDs | Detection |
//! |-------|-----|-----------|
//! | Binance diff-depth | `U` / `u` | full (overlap rule per Binance docs) |
//! | Bybit | `u` | contiguity |
//! | OKX | `prevSeqId` / `seqId` | contiguity |
//! | Crypto.com (delta subscription) | `pu` / `u` | contiguity |
//! | KuCoin futures level2 | `sequence` | contiguity |
//! | Kraken v2 | checksum only | none — deltas applied as-is |
//! | Coinbase l2 | none per-book | none — deltas applied as-is |
//!
//! Messages without IDs apply without sequence checks; the book still
//! handles snapshot/delta assembly and level removal.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::actors::OrderBookData;

// ── Price key ─────────────────────────────────────────────────────────────────

/// Total-order wrapper so `f64` prices can key a `BTreeMap`. Feed prices are
/// finite; `total_cmp` keeps any stray NaN from breaking map invariants.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Px(f64);

impl Eq for Px {}

impl PartialOrd for Px {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Px {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

// ── Apply outcome ─────────────────────────────────────────────────────────────

/// Outcome of feeding one [`OrderBookData`] into [`LocalOrderBook::apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookApply {
    /// A full snapshot replaced the book; it is now synced.
    Snapshot,
    /// An in-sequence delta was applied.
    Delta,
    /// Delta ignored — no snapshot has been applied yet (or the book was
    /// invalidated by a gap). Seed it with a snapshot first.
    AwaitingSnapshot,
    /// Delta ignored — its updates are entirely covered by the current
    /// state (e.g. buffered deltas from before a REST snapshot).
    Stale,
    /// The stream skipped ahead: the delta starts at `got` but the book
    /// expected `expected`. The book is now unsynced and will ignore
    /// further deltas until the next snapshot — fetch one and re-apply.
    Gap {
        /// The first update ID the book could have accepted (`last + 1`).
        expected: u64,
        /// The first update ID the delta actually carried.
        got: u64,
    },
}

impl BookApply {
    /// `true` when this outcome means the book lost sync and needs a
    /// fresh snapshot.
    #[must_use]
    pub const fn is_gap(self) -> bool {
        matches!(self, Self::Gap { .. })
    }
}

// ── LocalOrderBook ────────────────────────────────────────────────────────────

/// A locally maintained order book assembled from snapshot + delta messages.
///
/// See the [module docs](self) for the sync protocol and venue coverage.
#[derive(Debug, Clone, Default)]
pub struct LocalOrderBook {
    bids: BTreeMap<Px, f64>,
    asks: BTreeMap<Px, f64>,
    /// Last applied update ID, when the venue stamps one.
    last_update_id: Option<u64>,
    /// `false` until the first snapshot, and again after a gap.
    synced: bool,
    /// Symbol / exchange of the applied stream (captured from messages).
    symbol: String,
    exchange: String,
    /// Exchange timestamp of the most recently applied message (ms).
    exchange_ts: i64,
}

impl LocalOrderBook {
    /// An empty, unsynced book. Deltas are ignored until the first
    /// snapshot arrives.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one snapshot or delta message and report what happened.
    ///
    /// Snapshots always replace the book wholesale and re-sync it. Deltas
    /// are sequence-checked when both the book and the message carry
    /// update IDs:
    ///
    /// - `msg.last_update_id <= book.last_update_id` → [`BookApply::Stale`]
    ///   (skipped; normal when replaying buffered deltas over a snapshot).
    /// - `msg.first_update_id > book.last_update_id + 1` →
    ///   [`BookApply::Gap`] (book invalidated).
    /// - Otherwise the delta is applied — including Binance's overlapping
    ///   first-event-after-snapshot case (`U <= lastUpdateId + 1 <= u`).
    ///
    /// Levels with `qty == 0.0` are removed; all others are set to the
    /// given quantity (book deltas carry absolute sizes, not increments).
    pub fn apply(&mut self, msg: &OrderBookData) -> BookApply {
        if msg.is_snapshot {
            self.bids.clear();
            self.asks.clear();
            apply_levels(&mut self.bids, &msg.bids);
            apply_levels(&mut self.asks, &msg.asks);
            self.last_update_id = msg.last_update_id;
            self.synced = true;
            self.note_source(msg);
            return BookApply::Snapshot;
        }

        if !self.synced {
            return BookApply::AwaitingSnapshot;
        }

        if let (Some(prev), Some(last)) = (self.last_update_id, msg.last_update_id) {
            if last <= prev {
                return BookApply::Stale;
            }
            let first = msg.first_update_id.unwrap_or(last);
            if first > prev + 1 {
                self.synced = false;
                return BookApply::Gap {
                    expected: prev + 1,
                    got: first,
                };
            }
        }

        apply_levels(&mut self.bids, &msg.bids);
        apply_levels(&mut self.asks, &msg.asks);
        if msg.last_update_id.is_some() {
            self.last_update_id = msg.last_update_id;
        }
        self.note_source(msg);
        BookApply::Delta
    }

    /// Record symbol/exchange/timestamp from an applied message.
    fn note_source(&mut self, msg: &OrderBookData) {
        if self.symbol.is_empty() {
            self.symbol = msg.symbol.clone();
            self.exchange = msg.exchange.clone();
        }
        self.exchange_ts = msg.exchange_ts;
    }

    /// `true` once a snapshot has been applied and no gap has been seen
    /// since. While `false`, deltas are ignored.
    #[must_use]
    pub const fn is_synced(&self) -> bool {
        self.synced
    }

    /// The last applied update ID, when the venue stamps one.
    #[must_use]
    pub const fn last_update_id(&self) -> Option<u64> {
        self.last_update_id
    }

    /// Highest bid as `[price, qty]`.
    #[must_use]
    pub fn best_bid(&self) -> Option<[f64; 2]> {
        self.bids.last_key_value().map(|(p, q)| [p.0, *q])
    }

    /// Lowest ask as `[price, qty]`.
    #[must_use]
    pub fn best_ask(&self) -> Option<[f64; 2]> {
        self.asks.first_key_value().map(|(p, q)| [p.0, *q])
    }

    /// `best_ask - best_bid`, when both sides have depth.
    #[must_use]
    pub fn spread(&self) -> Option<f64> {
        Some(self.best_ask()?[0] - self.best_bid()?[0])
    }

    /// Midpoint of the best bid/ask, when both sides have depth.
    #[must_use]
    pub fn mid_price(&self) -> Option<f64> {
        Some(f64::midpoint(self.best_ask()?[0], self.best_bid()?[0]))
    }

    /// `true` when the best bid is at or above the best ask — a corrupted
    /// or transiently inconsistent book that shouldn't be traded against.
    #[must_use]
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => bid[0] >= ask[0],
            _ => false,
        }
    }

    /// Top `depth` bid levels as `[price, qty]`, best (highest) first.
    /// Pass `usize::MAX` for all levels.
    #[must_use]
    pub fn bids(&self, depth: usize) -> Vec<[f64; 2]> {
        self.bids
            .iter()
            .rev()
            .take(depth)
            .map(|(p, q)| [p.0, *q])
            .collect()
    }

    /// Top `depth` ask levels as `[price, qty]`, best (lowest) first.
    /// Pass `usize::MAX` for all levels.
    #[must_use]
    pub fn asks(&self, depth: usize) -> Vec<[f64; 2]> {
        self.asks
            .iter()
            .take(depth)
            .map(|(p, q)| [p.0, *q])
            .collect()
    }

    /// Number of bid price levels currently in the book.
    #[must_use]
    pub fn bid_depth(&self) -> usize {
        self.bids.len()
    }

    /// Number of ask price levels currently in the book.
    #[must_use]
    pub fn ask_depth(&self) -> usize {
        self.asks.len()
    }

    /// Export the current state as a full [`OrderBookData`] snapshot
    /// (e.g. to seed another consumer). `receipt_ts` is the export time.
    #[must_use]
    pub fn snapshot(&self) -> OrderBookData {
        OrderBookData {
            symbol: self.symbol.clone(),
            exchange: self.exchange.clone(),
            asks: self.asks(usize::MAX),
            bids: self.bids(usize::MAX),
            exchange_ts: self.exchange_ts,
            receipt_ts: chrono::Utc::now().timestamp_millis(),
            is_snapshot: true,
            first_update_id: self.last_update_id,
            last_update_id: self.last_update_id,
        }
    }
}

/// Set or remove levels: `qty == 0.0` removes, anything else overwrites.
fn apply_levels(side: &mut BTreeMap<Px, f64>, levels: &[[f64; 2]]) {
    for &[price, qty] in levels {
        if qty == 0.0 {
            side.remove(&Px(price));
        } else {
            side.insert(Px(price), qty);
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(
        is_snapshot: bool,
        bids: Vec<[f64; 2]>,
        asks: Vec<[f64; 2]>,
        ids: Option<(u64, u64)>,
    ) -> OrderBookData {
        OrderBookData {
            symbol: "BTCUSDT".into(),
            exchange: "test".into(),
            asks,
            bids,
            exchange_ts: 1_700_000_000_000,
            receipt_ts: 1_700_000_000_001,
            is_snapshot,
            first_update_id: ids.map(|(f, _)| f),
            last_update_id: ids.map(|(_, l)| l),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn delta_before_snapshot_is_ignored() {
        let mut book = LocalOrderBook::new();
        let out = book.apply(&msg(false, vec![[100.0, 1.0]], vec![], Some((1, 1))));
        assert_eq!(out, BookApply::AwaitingSnapshot);
        assert!(!book.is_synced());
        assert!(book.best_bid().is_none());
    }

    #[test]
    fn snapshot_then_contiguous_deltas() {
        let mut book = LocalOrderBook::new();
        let snap = msg(
            true,
            vec![[100.0, 1.0], [99.0, 2.0]],
            vec![[101.0, 1.5], [102.0, 3.0]],
            Some((10, 10)),
        );
        assert_eq!(book.apply(&snap), BookApply::Snapshot);
        assert!(book.is_synced());
        assert!(close(book.best_bid().unwrap()[0], 100.0));
        assert!(close(book.best_ask().unwrap()[0], 101.0));

        // Contiguous delta: new best bid + remove an ask level.
        let delta = msg(
            false,
            vec![[100.5, 0.7]],
            vec![[101.0, 0.0]],
            Some((11, 11)),
        );
        assert_eq!(book.apply(&delta), BookApply::Delta);
        assert!(close(book.best_bid().unwrap()[0], 100.5));
        assert!(close(book.best_ask().unwrap()[0], 102.0));
        assert_eq!(book.last_update_id(), Some(11));
        assert!(close(book.spread().unwrap(), 1.5));
        assert!(close(book.mid_price().unwrap(), 101.25));
        assert!(!book.is_crossed());
    }

    #[test]
    fn zero_qty_removes_level() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(true, vec![[100.0, 1.0]], vec![], Some((1, 1))));
        book.apply(&msg(false, vec![[100.0, 0.0]], vec![], Some((2, 2))));
        assert!(book.best_bid().is_none());
        assert_eq!(book.bid_depth(), 0);
    }

    #[test]
    fn stale_delta_is_skipped() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(true, vec![[100.0, 1.0]], vec![], Some((10, 10))));
        // Buffered delta from before the snapshot — must not change state.
        let out = book.apply(&msg(false, vec![[100.0, 9.0]], vec![], Some((9, 10))));
        assert_eq!(out, BookApply::Stale);
        assert!(close(book.best_bid().unwrap()[1], 1.0));
        assert_eq!(book.last_update_id(), Some(10));
    }

    #[test]
    fn binance_style_overlapping_first_delta_is_applied() {
        let mut book = LocalOrderBook::new();
        // REST snapshot at lastUpdateId = 10.
        book.apply(&msg(true, vec![[100.0, 1.0]], vec![], Some((10, 10))));
        // First stream event with U <= 11 <= u straddles the snapshot.
        let out = book.apply(&msg(false, vec![[100.0, 2.0]], vec![], Some((8, 12))));
        assert_eq!(out, BookApply::Delta);
        assert!(close(book.best_bid().unwrap()[1], 2.0));
        assert_eq!(book.last_update_id(), Some(12));
    }

    #[test]
    fn gap_invalidates_book_until_next_snapshot() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(true, vec![[100.0, 1.0]], vec![], Some((10, 10))));

        let out = book.apply(&msg(false, vec![[100.0, 2.0]], vec![], Some((13, 13))));
        assert_eq!(
            out,
            BookApply::Gap {
                expected: 11,
                got: 13
            }
        );
        assert!(out.is_gap());
        assert!(!book.is_synced());
        // The gapped delta must not have been applied.
        assert!(close(book.best_bid().unwrap()[1], 1.0));

        // Further deltas are ignored until a snapshot re-seeds the book.
        let out = book.apply(&msg(false, vec![[100.0, 3.0]], vec![], Some((14, 14))));
        assert_eq!(out, BookApply::AwaitingSnapshot);

        let out = book.apply(&msg(true, vec![[100.0, 4.0]], vec![], Some((20, 20))));
        assert_eq!(out, BookApply::Snapshot);
        assert!(book.is_synced());
        assert!(close(book.best_bid().unwrap()[1], 4.0));
    }

    #[test]
    fn unsequenced_streams_apply_without_gap_checks() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(true, vec![[100.0, 1.0]], vec![[101.0, 1.0]], None));
        let out = book.apply(&msg(false, vec![[99.0, 5.0]], vec![], None));
        assert_eq!(out, BookApply::Delta);
        assert_eq!(book.bid_depth(), 2);
    }

    #[test]
    fn levels_are_ordered_best_first() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(
            true,
            vec![[99.0, 1.0], [100.0, 2.0], [98.0, 3.0]],
            vec![[103.0, 1.0], [101.0, 2.0], [102.0, 3.0]],
            None,
        ));
        let bids = book.bids(2);
        let asks = book.asks(usize::MAX);
        assert!(close(bids[0][0], 100.0) && close(bids[1][0], 99.0));
        assert_eq!(bids.len(), 2);
        assert!(close(asks[0][0], 101.0) && close(asks[2][0], 103.0));
    }

    #[test]
    fn crossed_book_is_flagged() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(true, vec![[101.0, 1.0]], vec![[100.0, 1.0]], None));
        assert!(book.is_crossed());
    }

    #[test]
    fn snapshot_export_round_trips() {
        let mut book = LocalOrderBook::new();
        book.apply(&msg(
            true,
            vec![[100.0, 1.0], [99.0, 2.0]],
            vec![[101.0, 1.5]],
            Some((10, 10)),
        ));
        let exported = book.snapshot();
        assert!(exported.is_snapshot);
        assert_eq!(exported.symbol, "BTCUSDT");
        assert_eq!(exported.last_update_id, Some(10));

        let mut clone = LocalOrderBook::new();
        assert_eq!(clone.apply(&exported), BookApply::Snapshot);
        assert_eq!(clone.bids(usize::MAX), book.bids(usize::MAX));
        assert_eq!(clone.asks(usize::MAX), book.asks(usize::MAX));
    }
}
