//! REST API modules — market data, order management, account.

pub mod account;
pub mod market;
pub mod orders;

pub use account::{AccountOverview, FundingRecord, PositionInfo, RiskLimitLevel};
pub use market::{ContractInfo, FundingRate, MarkPrice, OrderBookSnapshot, Ticker};
pub use orders::{Fill, OrderDetail, OrderResponse, calc_contracts};
