# Enums Definitions

<Card>
### TimeInForce

Time in Force is a special strategy used during trading. It is used to specify how long an order shall remain active before being executed or expiring. 

Note: Order fills include self-fills. Market orders are not supported by the Time in Force strategy.

| Param | Description |
| --- | --- |
| GTC | Good Till Canceled, Expires only when canceled |
| GTT | Good Till Time, Expires at a specified time |
| IOC | Immediate Or Cancel, Execute the portions that can be executed immediately and cancel the rest; this does not enter the order book. |
| FOK | Fill Or Kill, Cancel if the order cannot be completely filled. |
| RPI | Retail Price Improvement, Provides better-than-market execution for retail orders. |

#### Retail Price Improvement (RPI)

:::warning[Warning]
Please note that the RPI feature is currently under internal development and testing, and has not been officially released for public use. In Phase 1, it supports Futures only (Spot and Margin are not supported). RPI currently supports Pro API Classic accounts only; UTA is not supported.
:::

RPI (Retail Price Improvement) order is a new type of order designed to provide retail traders with more targeted market liquidity and better execution prices.
RPI orders are subject to counterparty restrictions during matching and can only be executed against non-algorithmic, non-API retail orders. This mechanism aims to offer retail users a better trading experience while allowing market makers to engage in passive market making within a more controlled risk environment.Currently, only specific market maker partners are allowed to place RPI orders.

For more details, please refert to [What is an RPI Order](https://www.kucoin.com/support/48142946141511)

</Card>
<Card>
### STP (Self-Trade Prevention)

Self-Trade Prevention is an option in advanced settings. It is not selected by default. If you specify STP when placing orders, your order won't be matched by another one which is also yours. Conversely, if STP is not specified in advanced, your order can be matched by another one of your own orders. It should be noted that only the taker's protection strategy is effective.

For spot STP, it should be noted that the STP function only occurs at the UID level, and spot already supports the master account level STP whitelist function. If needed, please join our official API Telegram group: https://t.me/KuCoin_API, or contact your account manager.

For futures STP, STP defaults to the master-UID level. There is no need to enable the whitelist function. However, you need to carry the "stp" parameter when placing an order. After activating this, the master-UID and all sub-UIDs under the master-UID cannot run self-executions.

| Param | Description |
| --- | --- |
| DC | Decrease and Cancel, Cancel the order with smaller size and change the order with larger size to the difference between old and new |
| CO | Cancel old, Cancel the old order |
| CN | Cancel new, Cancel the new order |
| CB | Cancel both, Cancel both sides |
</Card>
<Card>
### Hidden Orders and Iceberg Orders (Hidden & Iceberg)

Hidden orders and iceberg orders can be set in advanced settings (iceberg orders are a special type of hidden order). When placing limit orders or stop limit orders, you can choose to execute according to either hidden orders or iceberg orders.

Hidden orders are not shown in order books.

Unlike hidden orders, iceberg orders are divided into visible and hidden portions. When engaging in iceberg orders, visible order sizes must be set. The minimum visible size for an iceberg order is 1/20 of the total order size.

When matching, the visible portions of iceberg orders are matched first. Once the visible portions are fully matched, hidden portions will emerge. This will continue until the order is fully filled.

Note:

- The system will charge taker fees for hidden orders and iceberg orders.
- If you simultaneously set iceberg orders and hidden orders, your order will default to an iceberg order for execution.

</Card>
