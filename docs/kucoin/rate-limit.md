# Rate Limit

<Card>
## RESTful Rate Limit

The specific rules of REST rate limit are as follows:

### 1. Resource Pool:
Each API resource pool has a certain quota, the specific amount of which depends on the VIP level:

Level  | Unified Account | Spot (include Margin)  | Futures  | Management  | Earn      | CopyTrading      | Public
-------|------|--------|----------|-------------|-----------|-----------|------
VIP0   | 200/3s| 4000/30s     | 2000/30s | 2000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP1   | 200/3s| 6000/30s     | 2000/30s | 2000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP2   | 400/3s| 8000/30s     | 4000/30s | 4000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP3   | 500/3s| 10000/30s    | 5000/30s | 5000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP4   | 600/3s| 13000/30s    | 6000/30s | 6000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP5   | 700/3s| 16000/30s    | 7000/30s | 7000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP6   | 800/3s| 20000/30s    | 8000/30s | 8000/30s    | 2000/30s  | 2000/30s  | 2000/30s
VIP7   | 1000/3s| 23000/30s    | 10000/30s| 10000/30s   | 2000/30s  | 2000/30s  | 2000/30s
VIP8   | 1200/3s| 26000/30s    | 12000/30s| 12000/30s   | 2000/30s  | 2000/30s  | 2000/30s
VIP9   | 1400/3s| 30000/30s    | 14000/30s| 14000/30s   | 2000/30s  | 2000/30s  | 2000/30s
VIP10  | 1600/3s| 33000/30s    | 16000/30s| 16000/30s   | 2000/30s  | 2000/30s  | 2000/30s
VIP11  | 1800/3s| 36000/30s    | 18000/30s| 18000/30s   | 2000/30s  | 2000/30s  | 2000/30s
VIP12  | 2000/3s| 40000/30s    | 20000/30s| 20000/30s   | 2000/30s  | 2000/30s  | 2000/30s

### 2. Weight: 
When a user requests any API, the weight of this interface will be deducted and updated every 30s (starting from the arrival time of the user's first request). For specific interfaces, please refer to the rate limit weight regulations under each interface. 

If the quota of any resource pool is used up within 30s, that is, after the rate limit is exceeded, an error message of http code:429, error code:429000 will be returned, and the request can be re-requested after the length of time displayed in the request header. At this point, the user must stop access and wait until the resource quota is reset before continuing to access.

For example:

When the user's VIP is 5, s/he has a "total spot quota" of 16000/30s.

The quota consumption for each "add spot limit order" is 2. After placing the first order, the user's remaining spot quota is 15998, after placing the second order, the remaining quota is 15996, and so on.

If the quota is not used up within 30 seconds, when the next cycle comes, the spot resource pool quota will be reset and returned to the quota limit of 16000.


### 3. Request Header:
The returned information of each request will carry the following information: Total resource pool quota, resource pool remaining quota, resource pool quota reset countdown (milliseconds).

"gw-ratelimit-limit": 500

"gw-ratelimit-remaining": 300

"gw-ratelimit-reset": 1489


### 4. Public Endpoint Rate Limit

It is **based on IP** rate limitation. If there is a lot of demand for the use of public interfaces, it is recommended to use the Websocket interface instead of the REST interface (if the interface supports it). To avoid IP rate limit issues, you may:

- Use one server to bind multiple IP addresses (IPv4 or IPv6).
- Use different IPs to avoid IP rate limit issues.


### 5. Private Endpoint Rate Limit

With the exception of **Public** resource pool endpoints, all other resource pools are **based on UID**, and the request header will carry the rate limit information of the resource pool, such as:

- Remaining rate limit times
- Rate limit cycle time

The rate limits of the sub-account and the master account are independent of each other at the API request level; that is to say, if the demand for such interface access is relatively large, it can also be solved by using the sub-account.

**Server Overload Rate Limit**

In addition to the regular rate limit, server overload may also trigger the rate limit. After the rate limit, the error code is **429000**, but the request header will not carry other personal rate limit information. This type of rate limit does not count toward the number of rate limits. It is recommended to try again later.

**Requesting Higher Limits**

If you are a professional trader or market maker and need a higher limit, please send your KuCoin account, reason and approximate trading volume to api@kucoin.com:

- KuCoin account information
- Reason for the request
- Estimated trading volume
</Card>

<Card>
## WebSocket Rate Limits

### 1. Maximum Concurrent Connections

| API | Limit | Scope | Note |
| --- | --- | --- | --- |
| Classic API | ≤ 800 concurrent connections | Private (authenticated) endpoints are counted by UID; Public (unauthenticated) endpoints are counted by IP | Master and sub-accounts are completely independent (different UIDs) |
| Pro API | ≤ 512 public + ≤ 512 private connections per IP (1024 total) | Counted by IP; public and private connections are counted separately |  |

### 2. Connection Establishment Rate

| API | Limit | Note |
| --- | --- | --- |
| Classic API | No limit |  |
| Pro API | 150 new connections per 5 minutes per IP | If exceeded, the server may reject new connection requests.<br>Reuse existing connections; avoid frequent disconnect/reconnect. |

### 3. Client-to-Server Messages (Client → Server)

| API | Limit | Scope / Notes |
| --- | --- | --- |
| Classic API | 100 messages per 10 seconds | Counted per single connection |
| Pro API | 100 messages per 10 seconds | Counted per single connection; includes `subscribe`, `unsubscribe`, `ping`.<br>WebSocket cancel-order messages are excluded.<br>If exceeded, the server may disconnect the connection. |

### 4. Subscribe / Unsubscribe Requests

| API | Max Topics per Request | Scope |
| --- | --- | --- |
| Classic API | 100 | Counted per single connection |
| Pro API | No limit | N/A |

### 5. Maximum Subscribed Topics per Connection

| API | Product Line | Limit | Note |
| --- | --- | --- | --- |
| Classic API | Spot / Margin | ≤ 400 topics |  |
| Classic API | Futures | No limit |  |
| Pro API | All | ≤ 200 topics | If more topics are required, split across multiple connections |

### 6. Usage Recommendations (Pro API)

1. Reuse existing WebSocket connections whenever possible.  
2. Control the client message sending rate (≤ 100 messages per 10 seconds per connection).  
3. Split topics across multiple connections when subscribing to a large number of topics.  
</Card>
