# Introduction

<Card>
Welcome to KuCoin’s trader and developer documentation. These documents outline the exchange functionality, market details, and APIs.
</Card>
<Card>
## Key Upgrades
Compared with the old version of the API documentation, the main changes are as follows:

- **Online Debugging Supported**
Each REST interface can be directly tested and run online—what you see is what you get. 

- **SDK Refactoring**
We have rewritten the [SDK](https://www.kucoin.com/docs-new/sdk.md) in all languages, with more standardized data specifications and a more convenient user experience.

- **Offline Data Download Supported**
Market data obtainable through [Offline Data Download](https://www.kucoin.com/markets/historydata) function to facilitate backtesting.

- **Readability Improvements to API Documentation**
The API documentation has been revised and proofread to improve clarity and reduce confusion. Any parts of the previous documentation that were not clear have been revised to provide better explanations.
</Card>
<Card>
## Document Structure
Under each endpoint, there are several modules, as follows:

### 1. domain
Prefix domain name. The request URL needs to be determined by BASE and the specific endpoint combination.

#### Unified Account

- **REST**: https://api.kucoin.com

- **Broker REST**: https://api-broker.kucoin.com

- **Spot & Margin Public Websocket Push**: wss://x-push-spot.kucoin.com

- **Futures Public Websocket Push**: wss://x-push-futures.kucoin.com

- **Private Websocket Push**: wss://wsapi-push.kucoin.com

- **Websocket Add/Cancel Order**: wss://wsapi.kucoin.com

#### Classic Account

- **Spot & Margin REST**: https://api.kucoin.com

- **Futures REST**: https://api-futures.kucoin.com

- **Broker REST**: https://api-broker.kucoin.com

- **Spot & Margin Websocket Push**: wss://ws-api-spot.kucoin.com

- **Futures Websocket Push**: wss://ws-api-futures.kucoin.com 

- **Websocket Add/Cancel Order**: wss://wsapi.kucoin.com



### 2. api-channel  
Private or Public endpoint. If it is a private interface, apikey is required to access it.

### 3. api-permission
You can manage the API permission on KuCoin’s official website. Please refer to the documentation below to see what API key permissions are required for a specific route. The permissions are:
- **General**: Limited to read-only operations such as querying account information, account statements, and order information. The API cannot be used to perform operations such as order placement or withdrawals.
- **Spot**: This API can be used for spot trading to perform order placements, order cancellations, etc.
- **Margin**: This API can be used for margin trading to perform order placements, order cancellations, etc.
- **Futures**: This API can be used for futures trading to perform order placements, order cancellations, etc.
- **Earn**: You may use this API to subscribe to, redeem, or make early redemptions of KuCoin Earn products.
- **Withdrawal**: This permission allows you to withdraw assets, acquire deposit addresses, cancel withdrawals, and execute other operations. To use this permission, you must enable IP Restriction. Please note that when authorizing the Transfer permission, you can use the API to transfer money without email verification or Google verification.
- **FlexTransfers**: Allow this API key for FlexTransfers. This will enable asset transfers across the multiple supported transfer types. The respective permissions for each individual transfer type remains unaffected. If this option is enabled, IP access restriction filters must also be applied.
- **LeadtradeFutures**: Allow this apikey to use the Futures CopyTrading function. Please note that only the TRADER (Lead Trader) need use this permission, not the FOLLOWER (Copy Trader).
- **Unified** ：Trade spot and derivatives simultaneously from a single account with multiple currencies, without needing to transfer between your different accounts.




### 4. api-rate-limit-pool
Each API resource pool has a certain quota, the specific amount of which depends on the VIP level. Please refer to [Resource pool](https://www.kucoin.com/docs-new/rate-limit.md)

### 5. api-rate-limit-weight
When a user requests any API, the weight of this interface will be deducted and updated every 30 seconds (starting from the arrival time of the user's first request), Please refer to [Weight](https://www.kucoin.com/docs-new/rate-limit.md).

### 6. sdk-service
The package name of this method in the SDK.

### 7. sdk-sub-service
The sub package name of this method in the SDK.

### 8. sdk-method-name
The method name of this method in the SDK.
</Card>
