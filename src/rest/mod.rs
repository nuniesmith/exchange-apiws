//! REST API modules — market data, order management, account.

pub mod account;
pub mod market;
pub mod orders;
pub mod uta;

pub use account::{
    AccountOverview, FundingRecord, PositionInfo, RiskLimitLevel, TransferRecord, TransferResponse,
};
pub use market::{ContractInfo, FundingRate, MarkPrice, OrderBookSnapshot, Ticker};
pub use orders::{Fill, OrderDetail, OrderResponse, StopOrderDetail};
pub use uta::{
    CrossMarginAccount, CrossMarginAsset, IsolatedMarginAccount, IsolatedMarginAsset,
    IsolatedMarginPair, UtaAccountSummary,
};
