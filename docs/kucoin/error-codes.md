# HTTP

| Code | Meaning                                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 400  | Bad Request -- Invalid request format.                                                                                               |
| 401  | Unauthorized -- Invalid API Key.                                                                                                     |
| 403  | Forbidden or Too Many Requests -- The request is forbidden or [Access limit breached](https://www.kucoin.com/docs-new/rate-limit.md). |
| 404  | Not Found -- The specified resource could not be found.                                                                              |
| 405  | Method Not Allowed -- You tried to access the resource with an invalid method.                                                       |
| 415  | Unsupported Media Type. You need to use: [application/json.](https://www.kucoin.com/docs-new/authentication.md#creating-a-request)                |
| 500  | Internal Server Error -- We had a problem with our server. Try again later.                                                          |
| 503  | Service Unavailable -- We're temporarily offline for maintenance. Please try again later.                                            |

# Spot

| Code   | Meaning |
| --- | --- |
| 102411 | Parameter error |
| 102421 | Insufficient account balance |
| 102423 | Trading direction not supported for fromCurrency |
| 102424 | Trading direction not supported for toCurrency |
| 102425 | Order time range not supported |
| 102426 | Duplicate user-defined unique order ID |
| 102427 | Number of pending orders exceeds the threshold |
| 102428 | User order is being processed and cannot be canceled |
| 102429 | User price is below the protection price |
| 102431 | Flash swap not supported for the trading pair |
| 102432 | Please provide either fromSize or toSize |
| 102434 | Exceeds the maximum order size for the currency |
| 102435 | Below the minimum order size for the currency |
| 102436 | Quote order does not exist or has expired |
| 102441 | The agreement for convert has not been signed. Please sign the agreement on the main account App/Web before using the service. |
|  110003 |  Withdrawal limit exceeded. The withdrawal request hit the limit. This is a 24-hour cumulative withdrawal amount restriction (total withdrawal cap within 24 hours). |
| 115007 | Insufficient account balance. Withdrawals are only supported from the funds account. Please transfer from your spot or other accounts to the funds account before proceeding with the withdrawal.                                                              |
| 200001 | Order creation for this pair suspended                                                                |
| 200002 | Order cancel for this pair suspended                                                                  |
| 200003 | Number of orders breached the limit                                                                   |
| 200009 | Please complete the KYC verification before you trade XX                                              |
| 200004 | Balance insufficient                                                                                  |
| 260210 | withdraw.disabled -- Currency/Chain withdraw is closed, or user is frozen to withdraw                 |
| 400001 | Any of KC-API-KEY, KC-API-SIGN, KC-API-TIMESTAMP, KC-API-PASSPHRASE is missing in your request header |
| 400002 | KC-API-TIMESTAMP Invalid                                                                              |
| 400003 | KC-API-KEY does not exist                                                                                 |
| 400004 | KC-API-PASSPHRASE error                                                                               |
| 400005 | Signature error                                                                                       |
| 400006 | The requested ip address is not on the api whitelist                                                  |
| 400007 | Access Denied                                                                                         |
| 404000 | Url Not Found                                                                                         |
| 400100 | Parameter Error                                                                                       |
| 400100 | account.available.amount -- Insufficient balance                                                      |
| 400200 | Forbidden to place an order                                                                           |
| 400500 | Your located country/region is currently not supported for the trading of this token                  |
| 400600 | validation.createOrder.symbolNotAvailable -- The trading pair has not yet started trading             |
| 400700 | Transaction restricted, there's a risk problem in your account                                        |
| 400800 | Leverage order failed                                                                                 |
| 411100 | User is frozen                                                                                       |
| 415000 | Unsupported Media Type -- The Content-Type of the request header needs to be set to application/json  |
| 500000 | Internal Server Error                                                                                 |
| 600203 | Symbol XXX-XXX cant be traded -- The symbol is not enabled for trading, such as downtime for upgrades, etc.    |
| 900001 | symbol does not exist                                                                                     |
| 230005 | The system is busy, please try again later                                                            |

If the returned HTTP status code is not 200, the error code will be included in the returned results. If the interface call is successful, the system will return the code and data fields. If not, the system will return the code and msg fields. You can check the error code for details.
# Margin

This table is the error code common to low-frequency margin and high-frequency margin.

| Code   | Meaning |
| ------ | ------ |
| 130101 | The currency does not support subscription.|
| 130101 | Interest rate increment error.                                                                                                                                         |
| 130101 | Interest rate exceeds limit.                                                                                                                                           |
| 130101 | The subscription amount exceeds the limit for a single subscription.                                                                                                   |
| 130101 | Subscription amount increment error.                                                                                                                                   |
| 130101 | Redemption amount increment error                                                                                                                                      |
| 130101 | Interest rate exceeds limit.                                                                                                                                           |
| 130102 | Maximum subscription amount has been exceeded.                                                                                                                         |
| 130103 | Subscription order does not exist.                                                                                                                                     |
| 130104 | Maximum number of subscription orders has been exceeded.                                                                                                               |
| 130105 | Insufficient balance.                                                                                                                                                  |
| 130106 | The currency does not support redemption.                                                                                                                              |
| 130107 | Redemption amount exceeds subscription amount.                                                                                                                         |
| 130108 | Redemption order does not exist.                                                                                                                                       |
| 130201 | Please open margin trade before proceeding                                                                                                                             |
| 130201 | Your account has restricted access to certain features. Please contact customer service for further assistance                                                         |
| 130201 | The lending function is currently disabled                                                                                                                             |
| 130202 | The system is renewing the loan automatically. Please try again later                                                                                                  |
| 130202 | The system is processing liquidation. Please try again later                                                                                                           |
| 130202 | Please pay off all debts before proceeding                                                                                                                             |
| 130202 | A borrowing is in progress. Please try again later                                                                                                                     |
| 130202 | A timeout has occurred. The system is currently processing                                                                                                             |
| 130202 | The system is renewing the loan automatically. Please try again later                                                                                                  |
| 130202 | The system is confirming position liquidation. Please try again later                                                                                                  |
| 130202 | The system is processing. Please try again later                                                                                                                       |
| 130202 | There are outstanding borrowing orders that need to be settled. Please try again later                                                                                 |
| 130203 | Insufficient account balance                                                                                                                                           |
| 130203 | The maximum borrowing amount has been exceeded. Your remaining available borrowing: {1}{0}                                                                             |
| 130204 | As the total lending amount for platform leverage {0} reaches the platform's maximum position limit, the system suspends the borrowing function of leverage {1}        |
| 130204 | As the total position of platform leverage {0} reaches the platform's maximum leverage loan limit, the system suspends leverage the borrowing function of leverage {1} |
| 130204 | According to the platform's maximum borrowing limit, the maximum amount you can borrow is {0}{1}                                                                       |
| 130301 | Insufficient account balance                                                                                                                                           |
| 130302 | Your relevant permission rights have been restricted. You can contact customer service for processing.                                                                  |
| 130303 | The current trading pair does not support isolated positions                                                                                                           |
| 130304 | The trading function of the current trading pair is not enabled                                                                                                        |
| 130305 | The current trading pair does not support cross position                                                                                                               |
| 130306 | The account has not opened leveraged trading                                                                                                                           |
| 130307 | Please reopen the leverage agreement                                                                                                                                   |
| 130308 | Position renewal freeze                                                                                                                                                |
| 130309 | Position forced liquidation freeze  |
| 130310 | Abnormal leverage account status |
| 130311 | Failed to place an order, triggering buy limit |
| 130312 | Trigger global position limit, suspend buying  |
| 130313 | Trigger global position limit, suspend selling |
| 130314 | Trigger the global position limit and prompt the remaining quantity available for purchase |
| 130315 | This feature has been suspended due to country restrictions   |
| 130316 | Unable to borrow and transfer assets simultaneously. Please try again late   |
|130323 | The leverage trading agreement has not been signed. Please sign the agreement on the main account App/Web before using the service|
 | 400400 | Parameter error/service exception |


This table is the error codes unique to margin high frequency

Code   | Meaning  
------ | ----------
126000 | Abnormal margin trading
126001 | Users currently do not support high frequency
126002 | There is a risk problem in your account and transactions are temporarily not allowed!
126003 | The commission amount is less than the minimum transaction amount for a single commission
126004 | Trading pair does not exist or is prohibited
126005 | This trading pair requires advanced KYC certification before trading
126006 | Trading pair is not available
126007 | Trading pair suspended
126009 | Trading pair is suspended from creating orders
126010 | Trading pair suspended order cancellation
126011 | There are too many orders in the order
126013 | Insufficient account balance
126015 | It is prohibited to place orders on this trading pair
126021 | This digital asset does not support user participation in your region, thank you for your understanding!
126022 | The final transaction price of your order will trigger the price protection strategy. To prevent the price from deviating too much, please place an order again.
126027 | Only limit orders are supported
126028 | Only limit orders are supported before the specified time
126029 | The maximum order price is: xxx
126030 | The minimum order price is: xxx
126033 | Duplicate order
126034 | Failed to create take profit and stop loss order
126036 | Failed to create margin order
126037 | Due to country and region restrictions, this function has been suspended!
126038 | Third-party service call failed (internal exception)
126039 | Third-party service call failed, reason: xxx
126041 | clientTimestamp parameter error
126042 | Exceeded maximum position limit
126043 | Order does not exist
126044 | clientOid duplicate
126045 | This digital asset does not support user participation in your region, thank you for your understanding!
126046 | This digital asset does not support your IP region, thank you for your understanding!
126047 | Please complete identity verification
126048 | Please complete authentication for the master account
135005 | Margin order query business abnormality
135018 | Margin order query service abnormality
400400 | Parameter error/service exception
400401 | User is not logged in
400444 | For your security, we have temporarily restricted your account function. Please contact support if you require further assistance with your account.
408000 | Network timeout, please try again later
500000 | Service exception



If the returned HTTP status code is not 200, the error code will be included in the returned results. If the interface call is successful, the system will return the code and data fields. If not, the system will return the code and msg fields. You can check the error code for details.
# Futures

| Code   | Meaning                                                                                                                                                                                                                                 |
| ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1015   | cloudflare frequency limit according to IP, block 30s                                                                                                                                                                                   |
| 40010  | Unavailable to place orders. Your identity information/IP/phone number shows you're in a country/region that is restricted from this service.                                                                                           |
| 100001 | There are invalid parameters                                                                                                                                                                                                            |
| 100002 | systemConfigError                                                                                                                                                                                                                       |
| 100003 | Contract parameter invalid                                                                                                                                                                                                              |
| 100004 | Order is in not cancelable state                                                                                                                                                                                                        |
| 100005 | contractRiskLimitNotExist                                                                                                                                                                                                               |
| 200001 | The query scope for Level 2 cannot exceed xxx                                                                                                                                                                                           |
| 200002 | Too many requests in a short period of time, please retry later--KuCoin business layer request frequency limit, block 10s                                                                                                               |
| 200002 | The query scope for Level 3 cannot exceed xxx                                                                                                                                                                                           |
| 200003 | The symbol parameter is invalid.                                                                                                                                                                                                        |
| 200005 | Insufficient balance (insufficient balance when modifying risk limit)                                                                                                                                                                                                      |
| 300000 | Request parameter illegal                                                                                                                                                                                                               |
| 300001 | Active order quantity limit exceeded (limit: xxx; current: xxx)                                                                                                                                                                         |
| 300002 | Order placement/cancellation suspended, please try again later.                                                                                                                                                                         |
| 300003 | Balance not enough, please first deposit at least 2 USDT before you go to battle                                                                                                                                                    |
| 300004 | Stop order quantity limit exceeded (limit: xxx, current: xxx)                                                                                                                                                                           |
| 300005 | The order will exceed the maximum risk limit of xxx.                                                                                                               |
| 300006 | The close price shall be greater than the bankruptcy price. Current bankruptcy price: xxx.                                                                                                                                              |
| 300007 | priceWorseThanLiquidationPrice                                                                                                                                                                                                          |
| 300008 | Unavailable to place the order, there's no contra order in the market.                                                                                                                                                                  |
| 300009 | Current position size: 0, unable to close the position.                                                                                                                                                                                 |
| 300010 | Failed to close the position                                                                                                                                                                                                            |
| 300011 | Order price cannot be higher than xxx                                                                                                                                                                                                   |
| 300012 | Order price cannot be lower than xxx                                                                                                                                                                                                    |
| 300013 | Unable to proceed the operation, there's no contra order in orderbook.                                                                                                                                                                 |
| 300014 | The position is being liquidated, unable to place/cancel the order. Please try again later.                                                                                                                                             |
| 300015 | The order placing/cancellation is currently not available. The Contract/Funding is undergoing the settlement process. When the process has been completed, the function will be restored automatically. Please be patient and try again later. |
| 300016 | The leverage cannot be greater than xxx.                                                                                                                                                                                                |
| 300017 | Unavailable to proceed the operation, this position is for Futures Brawl                                                                                                                                                                |
| 300018 | clientOid parameter repeated                                                                                                                                                                                                            |
| 330005 | Please switch your margin mode to cross margin mode and try again                                                                                              |
| 330011 | Before initiating any Futures trades, please use the Switch Position Mode endpoint to update your position mode and confirm the change using the Get Position Mode endpoint.|
| 400001 | Any of KC-API-KEY, KC-API-SIGN, KC-API-TIMESTAMP, KC-API-PASSPHRASE is missing in your request header.                                                                                                                                  |
| 400002 | KC-API-TIMESTAMP Invalid -- Time differs from server time by more than 5 seconds                                                                                                                                                        |
| 400003 | KC-API-KEY does not exist                                                                                                                                                                                                                   |
| 400004 | KC-API-PASSPHRASE error                                                                                                                                                                                                                 |
| 400005 | Signature error -- Please check your signature                                                                                                                                                                                          |
| 400006 | The IP address is not on the API whitelist                                                                                                                                                                                              |
| 400007 | Access Denied -- Your API key does not have sufficient permissions to access the URI                                                                                                                                                    |
| 400100 | Parameter Error -- You tried to access the resource with invalid parameters                                                                                                                                                             |
| 400100 | account.available.amount -- Insufficient balance                                                                                                                                                                                        |
| 404000 | URL Not Found -- The requested resource could not be found                                                                                                                                                                              |
| 411100 | User is frozen -- Please contact us via support center                                                                                                                                                                                  |
| 415000 | Unsupported Media Type -- The Content-Type of the request header needs to be set to application/json                                                                                                                                    |
| 429000 | Too Many Requests -- The total traffic limit of this KuCoin server interface has been triggered, you can retry the request                                                                                                                      |
| 500000 | Internal Server Error -- We had a problem with our server. Try again later.                                                                                                                                                             |

If the returned HTTP status code is not 200, the error code will be included in the returned results. If the interface call is successful, the system will return the code and data fields. If not, the system will return the code and msg fields. You can check the error code for details.
# Earn

| Code  | Meaning |
| ------- | ----------------------------------------- |
| 151404 | Position does not exist |
| 151001 | Product does not exist |
| 151002 | Subscription not started |
| 151003 | Subscription ended |
| 151004 | Subscription amount is less than the user's minimum subscription quota |
| 151005 | Subscription amount exceeds the user's maximum subscription quota |
| 151006 | Product is fully subscribed |
| 151007 | Only new users can participate in the subscription |
| 151008 | You currently do not meet the conditions to purchase this product |
| 151009 | Insufficient balance |
| 151010 | Quantity precision is incorrect |
| 151011 | Sorry, the current activity is too popular, please try again later |
| 151016 | Cannot redeem before the product expires |
| 151018 | Redemption amount exceeds the redeemable amount |
| 151019 | Remaining holding quantity is too small to generate income, please redeem all |
| 151020 | Redeemable quantity is less than the quantity required for penalty interest |
| 151021 | ETH Staking: This currency is not supported at the moment |
| 151022 | ETH Staking: Less than the minimum subscription quantity |
| 151023 | ETH Staking: The product is temporarily sold out, please wait for quota replenishment |
| 151024 | When redeeming early, the parameter confirmPunishRedeem must be passed in |

# Broker

Code   | Meaning  
------ | -----
100001 | downloading -- The rebate order file is being downloaded, please try again later.
400100 | request parameter illegal
600100 | broker not exists
600101 | account not exists
600102 | Your account has been disabled.
600100 | Your account has been frozen.
600120 | Illegal permission -- When creating a sub-account apikey, the permissions are incorrect.

# CopyTrading

Code    | Meaning
------- | ---
180001  | Request parameter illegal
180011  | Symbol not supported for copyTrading
180012  | Risk limit level exceeds the maximum limit.
180013  | leverage exceeds the maximum allowed limit
180014  | order size must be greater than 0
180200  | Unsupported contract
180201  | Trading expert does not exist
180202  | Trading expert level error
180203  | The limit order price cannot be empty
180204  | Copy trading futures does not support cross margin orders currently. Please use isolated margin orders instead.
180205  | info.createOrder.transferAmount
180206  | error.createOrder.leverageIsTooLarge
180207  | "Leverage must be greater than 0
180208  | error.createOrder.costLimit
1015    | cloudflare frequency limit according to IP, block 30s
40010   | Unavailable to place orders. Your identity information/IP/phone number shows you're at a country/region that is restricted from this service.
100001    | There are invalid parameters
100002    | systemConfigError
100003    | Contract parameter invalid
100004    | Order is in not cancelable state
100005    | contractRiskLimitNotExist
200001    | The query scope for Level 2 cannot exceed xxx
200002    | Too many requests in a short period of time, please retry later
200002    | The query scope for Level 3 cannot exceed xxx
200003    | The symbol parameter is invalid.
200005    | Insufficient balance.
300000    | Request parameter illegal
300001    | Active order quantity limit exceeded (limit: xxx, current: xxx)
300002    | Order placement/cancellation suspended, please try again later.
300003    | Balance not enough, please first deposit at least 2 USDT before you go to battle
300004    | Stop order quantity limit exceeded (limit: xxx, current: xxx)
300005    | xxx risk limit exceeded
300006    | The close price shall be greater than the bankruptcy price. Current bankruptcy price: xxx.
300007    | priceWorseThanLiquidationPrice
300008    | Unavailable to place the order, there's no contra order in the market.
300009    | Current position size: 0, unable to close the position.
300010    | Failed to close the position
300011    | Order price cannot be higher than xxx
300012    | Order price cannot be lower than xxx
300013    | Unable to proceed the operation, there's no contra order in order book.
300014    | The position is being liquidated, unable to place/cancel the order. Please try again later.
300015    | The order placing/cancellation is currently not available. The Contract/Funding is undergoing the settlement process. When the process has been completed, the function will be restored automatically. Please be patient and try again later
300016    | The leverage cannot be greater than xxx
300017    | Unavailable to proceed with the operation; this position is for Futures Brawl
300018    | clientOid parameter repeated
400001    | Any of KC-API-KEY, KC-API-SIGN, KC-API-TIMESTAMP, KC-API-PASSPHRASE is missing in your request header
400002    | KC-API-TIMESTAMP Invalid
400003    | KC-API-KEY does not exist
400004    | KC-API-PASSPHRASE error
400005    | Signature error
400006    | The requested IP address is not on the api whitelist
400007    | Access Denied
400100    | Parameter Error
400100    | account.available.amount
404000    | Url Not Found
411100    | User is frozen
415000    | Unsupported Media Type
429000    | Too Many Requests
500000    | Internal Server Error

If the returned HTTP status code is not 200, the error code will be included in the returned results. If the interface call is successful, the system will return the code and data fields. If not, the system will return the code and msg fields. You can check the error code for details.

# Websocket


## ERROR CODE
### Possible error codes when subscribing
| Code | Meaning                                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 400  | topic is invalid |
| 403  | login is required   |
| 404  | topic does not exist |
| 406  | topic is required|
| 509  |exceed max subscription count limitation of 100 per time|
| 509  | exceed max subscription count limitation of 300 per session |

### Possible error codes when establishing WebSocket connection
| Code | Meaning                                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 401  | token is invalid |
| 509  | exceed max session count limitation of 50 |
| 509  | service busy, please retry later |


### Other error codes:
| Code | Meaning                                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 400  | ping timeout |
| 415  | command type is invalid  |
| 500  | system error  |
| 509  | exceed max permits per second|




## GATEWAY ERROR CODE
### 1. Request Errors (`400xxx`)

| Code     | Message |
|----------|---------|
| `400001` | Please check the URL of your request. |
| `400002` | Invalid `KC-API-TIMESTAMP`. |
| `400003` | `KC-API-KEY` does not exist. |
| `400004` | Invalid `KC-API-PASSPHRASE`. |
| `400005` | Invalid `KC-API-SIGN`. |
| `400006` | Invalid request IP, the current client IP is: `%s`. |
| `400007` | Access denied, requires more permission. |
| `400008` | V1 & V2 API keys are no longer supported by this API. Please create a V3 API key. |
| `400009` | Invalid `KC-API-KEY-VERSION`. |
| `400010` | UID access denied, requires more permission. |
| `400011` | Session verification failed. (After the server returns the sessionId, the client must sign the request with its secret and send it back, but the signature is incorrect and does not match what the server expects.)|
| `400012` | Session verification has timed out.  (After the server returns the sessionId, the client did not send the signed request within the allowed time window (e.g. 30 seconds), so the server aborted verification.)|

### 2. Partner Errors (`4002xx`)

| Code     | Message |
|----------|---------|
| `400200` | Unknown partner. |
| `400201` | Invalid `KC-API-PARTNER-SIGN`. |
| `400202` | Invalid request IP. |

### 3. Regional & KYC Limitations (`4003xx`)

| Code     | Message |
|----------|---------|
| `400301` | Operation restricted due to local laws, regulations, or policies in your country or region. |
| `400302` | Based on your IP, services are unavailable in your region due to regulations. Please contact support. |
| `400303` | Identity verification required to access full features. |
| `400304` | Please log in with your master account to complete identity verification. |

### 4. Authorization Errors (`4004xx`)

| Code     | Message |
|----------|---------|
| `400400` | Invalid authorization token. |
| `400401` | Authorization is required. |

### 5. Data Errors (`4001xx`)

| Code     | Message |
|----------|---------|
| `400101` | Invalid request data. |
| `400102` | Please check the parameters of your request. |

### 6. Websocket Errors to disconnect
| Code     | Message |
|----------|---------|
| `420001` | Too many errors, disconnected. Please try again later. |
| `420002` | Error receiving data. |

### 7. Rate Limiting & Frequency Errors (`429xxx`)

| Code     | Message |
|----------|---------|
| `429000` | Too many requests in a short period. Please retry later.(UID LIMIT) |
| `429001` | Too many total requests in a short period. Please retry later.(SYSTEM LIMIT) |
| `429002` | Too many requests in a short period. Please retry later. (MULTY LIMIT PER CONNECTION)|

### 8. User Restriction Errors (`411xxx`)

| Code     | Message |
|----------|---------|
| `411200` | URL is in the user blacklist. |

### 9. Server Errors (`5xxxxx`)

| Code     | Message |
|----------|---------|
| `500000` | Internal server error. |
| `501000` | The service has stopped running. Please disconnect: exiting|
| `503000` | Server is busy. Please retry later. |
| `504000` | Gateway timeout. |
| `505000` | Unknown error. |

# Pro API

| Code   | Meaning                                                                                               |
| ------ | ----------------------------------------------------------------------------------------------------- |
| 4001     | Common Param Invalid                                                        |
| 106000   | InvalidParam                                                                |
| 106001   | InvalidAccount                                                              |
| 106050   | Canceling                                                                   |
| 106052   | NotFound                                                                    |
| 106053   | SymbolForbidden                                                             |
| 106054   | InvalidSymbol                                                               |
| 106100   | MissingParameter                                                            |
| 106100   | NotFoundOrderId                                                             |
| 106102   | NotFoundClientOrderId                                                       |
| 106103   | ExceedMaxRequestSize                                                        |
| 106150   | InsufficientFunds                                                           |
| 106151   | DuplicateClientOrderId                                                      |
| 106152   | DuplicateOrderId                                                            |
| 106155   | NoFundsMarket                                                               |
| 106156   | NoQuantityMarket                                                            |
| 106157   | BothFundsAandQuantityMarket                                                 |
| 106160   | OrderMissingMarkPrice                                                       |
| 106161   | NoPrice                                                                     |
| 106162   | NoQuantity                                                                  |
| 106164   | QuantityTooLow                                                              |
| 106165   | QuantityTooHigh                                                             |
| 106166   | FundsTooLow                                                                 |
| 106167   | FundsTooHigh                                                                |
| 106168   | BaseStepSizeMismatch                                                        |
| 106169   | PriceStepSizeMismatch                                                       |
| 106170   | PriceTooHigh                                                                |
| 106171   | PriceTooLow                                                                 |
| 106172   | FundsStepSizeMismatch                                                       |
| 106173   | ValueOverflow                                                               |
| 106174   | LeverageOverSymbolMax                                                       |
| 106176   | BookEmpty                                                                   |
| 106182   | NoPosition                                                                  |
| 106183   | MissingLeverageForPlaceOrder                                                |
| 106190   | UnknownSymbol                                                               |
| 106193   | UnknownCurrency                                                             |
| 106194   | SymbolDisabled                                                              |
| 106204   | SystemInternalError                                                         |
| 106205   | UnsupportedTradeType                                                        |
| 106206   | OpenOrdersExceededMaxLimit                                                  |
| 106208   | UserStatusIsPauseTrading                                                    |
| 106209   | SystemOrderIdempotent                                                       |
| 106210   | AdvancedOrderCanceled                                                       |
| 106211   | UserStatusNotSupportTrading                                                 |
| 106212   | InsufficientFundsOrRiskLimit                                                |
| 109000   | USER_NOT_EXIST                                                              |
| 109001   | ACCOUNT_TYPE_INVALID                                                        |
| 109002   | ACCOUNT_OPEN_STATUS_INVALID                                                 |
| 109003   | ACCOUNT_OPEN_STATUS_ILLEGAL                                                 |
| 109004   | PRE_CONFIRMED_NOT_PASSED                                                    |
| 109005   | USER_STATUS_CHECK_NOT_PASSED                                                |
| 109006   | SWITCH_ACCOUNT_OPEN_STATUS_FAIL                                             |
| 109008   | CURRENCY_INVALID                                                            |
| 109009   | RPC_CALL_FAIL                                                               |
| 109010   | USER_NOT_IN_GRAY                                                            |
| 109011   | SWITCH_CHECK_FAIL                                                           |
| 109099   | SYSTEM_ERROR                                                                |
| 110000   | INNER_SYSTEM_ERROR                                                          |
| 110001   | SYSTEM_BUSY                                                                 |
| 110002   | PARAM_ERROR                                                                 |
| 110003   | BIZ_ERROR                                                                   |
| 110106   | QUERY_RISK_ERROR                                                            |
| 110201   | USER_NOT_LOGGED_IN                                                          |
| 110202   | USER_NOT_EXISTS                                                             |
| 110203   | ACCOUNT_NOT_EXIST                                                           |
| 110204   | CONTRACT_NOT_EXIST                                                          |
| 112001   | RequestTimeout                                                              |
| 112002   | ServiceUnavailable                                                          |
| 112003   | InternalServerError                                                         |
| 301100   | UserRestrictLimit                                                           |
| 301101   | UserIpLimit                                                                 |
| 301102   | KYCLimit                                                                    |
| 301103   | KYCLimitSub                                                                 |
| 301105   | CreateOrderError                                                            |
| 301106   | SymbolNotAvailable                                                          |
| 301113   | OnlySupportLimitOrder                                                       |
| 301114   | OnlySupportLimitOrderError                                                  |
| 301115   | PriceGreaterThanUpPrice                                                     |
| 301116   | PriceLessThanDownPrice                                                      |
| 301117   | BizLimit                                                                    |
| 301118   | CALL_AUCTION_SEC_STAGE_FORBIDDEN                                            |
| 301119   | CALL_AUCTION_TYPE_NOT_SUPPORTED                                             |
| 301120   | CALL_AUCTION_PRICE_INVALID                                                  |
| 301121   | CALL_AUCTION_THIRD_STAGE_CANCEL_FORBIDDEN                                   |
| 301122   | CALL_AUCTION_THIRD_STAGE_DEAL_FORBIDDEN                                     |
| 301123   | CALL_AUCTION_WHITE_LIST_FORBIDDEN                                           |
| 301124   | USER_NOT_LOGGED_IN                                                          |
| 301126   | COMMISSION_FUNDS_LESS_THEN_MINIMUM_ORDER_FUNDS                              |
| 301127   | SYMBOL_KYC_ERROR                                                            |
| 301128   | SYMBOL_CAN_NOT_TRADED                                                       |
| 301129   | GET_MARKET_PRICE_FAILED                                                     |
| 301130   | ACCOUNT_BALANCE_INSUFFICIENT                                                |
| 301133   | IP_RESTRICTION                                                              |
| 301135   | PAUSE_CANCEL                                                                |
| 301136   | QUERY_LIVE_ORDER_ERROR                                                      |
| 401001   | USER_FUTURES_TRADE_STATUS_INVALID                                           |
| 401100   | ContractNotExist                                                            |
| 401101   | MarketOrderNotAllowed                                                       |
| 401102   | PriceIsTooHigh                                                              |
| 401103   | PriceIsTooLow                                                               |
| 401106   | ContractNationalRisk                                                        |
| 401107   | NoKycOrderNotAllowed                                                        |
| 401108   | NoKycOnlyCloseOrderAllowed                                                  |
| 401109   | OnlyReduceAllowedOnLow                                                      |
| 401110   | MarginModeValidIsolatedError                                                |
| 401111   | ContractStatusOnReduceOnly                                                  |
| 401112   | MarkPriceNotExist                                                           |
| 401113   | TradePriceNotExist                                                          |
| 401115   | ADVANCED_ORDER_TRIGGER_DIRECTION_ILLEGAL                                    |
| 401116   | ADVANCED_ORDER_TRIGGER_PRICE_TYPE_ILLEGAL                                   |
| 401117   | TradeTypeInvalid                                                            |
| 401118   | PositionPartiallyClosed                                                     |
| 401119   | PositionCloseFailed                                                         |
| 401121   | ADVANCED_ORDER_TRIGGER_PRICE_ILLEGAL                                        |
| 401122   | ADVANCED_ORDER_TP_ORDER_PRICE_ILLEGAL                                       |
| 401123   | ADVANCED_ORDER_TP_TRIGGER_PRICE_TYPE_ILLEGAL                                |
| 401124   | ADVANCED_ORDER_TP_TRIGGER_PRICE_ILLEGAL                                     |
| 401125   | ADVANCED_ORDER_SL_ORDER_PRICE_ILLEGAL                                       |
| 401126   | ADVANCED_ORDER_SL_TRIGGER_PRICE_TYPE_ILLEGAL                                |
| 401127   | ADVANCED_ORDER_SL_TRIGGER_PRICE_ILLEGAL                                     |
| 401128   | PRE_MARKET_ORDER_FORBIDDEN                                                  |
| 401200   | USER_RESTRICT_BUSINESS_HANDLE_FAILED                                        |
| 600000   | TAX_PAN_NUMBER_EMPTY                                                        |
| 600005   | TAX_UNPAID_ERROR                                                            |
| 603000   | Loan Param Invalid                                                          |
| 603001   | UserNotExist                                                                |
| 603002   | UnknownCurrency                                                             |
| 603005   | CallRemoteException                                                         |
| 603006   | InnerQueryError                                                             |
| 603007   | ServiceError                                                                |
| 603008   | RepayProcessingError                                                        |
| 706000   | UserNotExist                                                                |
| 706001   | SwitchAccountOpenStatusFail                                                 |
| 706002   | ParamInvalid                                                                |
| 706003   | PreConfirmCheckFail                                                         |
