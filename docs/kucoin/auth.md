# Authentication

<Card>
## Authentication

### Generating an API Key

Before being able to sign any requests, you must create an API key via the [KuCoin website](https://www.kucoin.com/account/api). Upon creating a key you need to write down 3 pieces of information:

- Key
- Secret
- Passphrase

The Key and Secret are generated and provided by KuCoin and the Passphrase refers to the one you used to create the KuCoin API. Please note that these three pieces of information can not be recovered once lost. If you lost this information, please create a new API key.

### API KEY PERMISSIONS

You can manage the API permission on KuCoin’s official website. Please refer to the documentation below to see what API key permissions are required for a specific route.

### Creating a Request

All private REST requests must include the required headers listed below. You may also include optional headers when applicable.
| Header | Required | Description |
| --- | --- | --- |
| **KC-API-KEY** | Yes | Your API key, provided as a string. |
| **KC-API-SIGN** | Yes | The Base64-encoded signature generated for the request. |
| **KC-API-TIMESTAMP** | Yes | The timestamp of the request in milliseconds. |
| **KC-API-PASSPHRASE** | Yes | The passphrase specified when the API key was created. |
| **KC-API-KEY-VERSION** | Yes | The API key version. This value can be checked on the API Management page. |
| **Content-Type** | Yes | All requests and responses use the `application/json` content type. |
| **X-SITE-TYPE** | No | Used to specify the site/region for public APIs. If not provided, the default value is `global`. For Australia site users, set this value to `australia` to ensure the returned data corresponds to the Australia site. For WebSocket connections, this header is also required during the token acquisition step when site differentiation is needed. |
    

### Signing a Message

For the header of **KC-API-SIGN**:

- Use API-Secret to encrypt the prehash string {timestamp+method+endpoint+body} with sha256 HMAC. The request body is a JSON string and need to be the same with the parameters passed by the API.
- Encode contents by **base64** before you pass the request.

For the **KC-API-PASSPHRASE** of the header:

- Encrypt passphrase with **HMAC-sha256** via API-Secret.
- Encode contents by **base64** before you pass the request.

Note:

- The encrypted timestamp shall be consistent with the KC-API-TIMESTAMP field in the request header.
- The body to be encrypted shall be consistent with the content of the Request Body.
- The Method should be UPPER CASE.
- For GET, DELETE request, all query parameters need to be included in the request url. e.g. /api/v1/deposit-addresses?currency=XBT. The body is "" if there is no request body (typically for GET requests).
- For the POST request, all query parameters need to be included in the request body with JSON. (e.g. {"currency":"BTC"}). Do not include extra spaces in JSON strings.
- When generating signature, the URL must use the content that has not been URL-encoded to participate in the signature.
For example: When the url is /api/v1/sub/api-key?apiKey=67*b3&subName=test&passphrase=abc%21%40%2311
, the url content participating in the signature should be the original information /api/v1/sub/api-key?apiKey=67*b3&subName=test&passphrase=abc!@#11
    
### Code Examples

<AccordionGroup>
<Accordion title="Python">
For a more production-ready implementation, please refer to: [Code](https://github.com/Kucoin/kucoin-universal-sdk/blob/main/sdk/python/kucoin_universal_sdk/internal/infra/default_signer.py)
  ```Python 
import base64
import hashlib
import hmac
import json
import logging
import os
import time
import uuid
import requests
from urllib.parse import urlencode


class KcSigner:
    def __init__(self, api_key: str, api_secret: str, api_passphrase: str,
                 broker_partner: str = "", broker_name: str = "", broker_key: str = ""):
        """
        KcSigner contains information about 'apiKey', 'apiSecret', 'apiPassPhrase'
        and provides methods to sign and generate headers for API requests.
        """
        self.api_key = api_key or ""
        self.api_secret = api_secret or ""
        self.api_passphrase = api_passphrase or ""
        self.broker_partner = broker_partner or ""
        self.broker_name = broker_name or ""
        self.broker_key = broker_key or ""

        # Encrypt passphrase
        if api_passphrase and api_secret:
            self.api_passphrase = self.sign(
                api_passphrase.encode('utf-8'),
                api_secret.encode('utf-8')
            )

        if not all([api_key, api_secret, api_passphrase]):
            logging.warning("API token is empty. Access is restricted to public interfaces only.")

    def sign(self, plain: bytes, key: bytes) -> str:
        hm = hmac.new(key, plain, hashlib.sha256)
        return base64.b64encode(hm.digest()).decode()

    def headers(self, plain: str) -> dict:
        """
        Generate signature headers for API authorization.
        """
        timestamp = str(int(time.time() * 1000))
        signature = self.sign((timestamp + plain).encode("utf-8"), self.api_secret.encode("utf-8"))

        headers = {
            "KC-API-KEY": self.api_key,
            "KC-API-PASSPHRASE": self.api_passphrase,
            "KC-API-TIMESTAMP": timestamp,
            "KC-API-SIGN": signature,
            "KC-API-KEY-VERSION": "3"
            "X-SITE-TYPE": "global"  /* 
            Site identifier.
            - Use "global" by default.
            - Set to "australia" when accessing Australia site data or services.
            */
        }

        # Add broker headers if all parameters are provided
        if self.broker_partner and self.broker_name and self.broker_key:
            message = timestamp + self.broker_partner + self.api_key
            partner_sign = base64.b64encode(
                hmac.new(self.broker_key.encode("utf-8"), message.encode("utf-8"), hashlib.sha256).digest()
            ).decode()

            headers.update({
                "KC-API-PARTNER": self.broker_partner,
                "KC-API-PARTNER-SIGN": partner_sign,
                "KC-BROKER-NAME": self.broker_name,
                "KC-API-PARTNER-VERIFY": True
            })

        return headers


def process_headers(signer: KcSigner, body: bytes, raw_url: str,
                    request: requests.PreparedRequest, method: str):
    request.headers["Content-Type"] = "application/json"

    payload = method + raw_url + body.decode()
    headers = signer.headers(payload)
    request.headers.update(headers)


def get_trade_fees(signer: KcSigner, session: requests.Session):
    endpoint = "https://api.kucoin.com"
    path = "/api/v1/trade-fees"
    method = "GET"
    query_params = {"symbols": "BTC-USDT"}

    full_path = f"{endpoint}{path}?{urlencode(query_params)}"
    raw_url = f"{path}?{urlencode(query_params)}"

    req = requests.Request(method=method, url=full_path).prepare()
    process_headers(signer, b"", raw_url, req, method)

    resp = session.send(req)
    print(json.loads(resp.content))


def add_limit_order(signer: KcSigner, session: requests.Session):
    endpoint = "https://api.kucoin.com"
    path = "/api/v1/hf/orders"
    method = "POST"

    order_data = json.dumps({
        "clientOid": str(uuid.uuid4()),
        "side": "buy",
        "symbol": "BTC-USDT",
        "type": "limit",
        "price": "100000",
        "size": "0.00001"
    })

    full_path = f"{endpoint}{path}"
    raw_url = path

    req = requests.Request(method=method, url=full_path, data=order_data).prepare()
    process_headers(signer, order_data.encode(), raw_url, req, method)

    resp = session.send(req)
    print(json.loads(resp.content))


def add_futures_limit_order(signer: KcSigner, session: requests.Session):
    endpoint = "https://api-futures.kucoin.com"
    path = "/api/v1/orders"
    method = "POST"

    order_data = json.dumps({
        "clientOid": str(uuid.uuid4()),
        "side": "buy",
        "symbol": "XBTUSDTM",
        "type": "limit",
        "price": "91000",
        "size": 1,
        "leverage": "5",
        "marginMode": "CROSS",
        "reduceOnly": False,
        "timeInForce": "GTC"
    })

    full_path = f"{endpoint}{path}"
    raw_url = path

    req = requests.Request(method=method, url=full_path, data=order_data).prepare()
    process_headers(signer, order_data.encode(), raw_url, req, method)

    resp = session.send(req)
    resp_obj = json.loads(resp.content)
    print(resp_obj)


def query_broker_user(signer: KcSigner, session: requests.Session):
    """
    Call Broker API: GET /api/v2/broker/queryUser
    No request parameters required.
    """
    endpoint = "https://api.kucoin.com"
    path = "/api/v2/broker/queryUser"
    method = "GET"

    full_path = f"{endpoint}{path}"
    raw_url = path  # No query params, so raw_url is just the path

    # Prepare request
    req = requests.Request(method=method, url=full_path).prepare()

    # No body for GET requests
    process_headers(signer, b"", raw_url, req, method)

    # Send request
    resp = session.send(req)
    print(json.loads(resp.content))


def query_broker_info(signer: KcSigner, session: requests.Session):
    """
    Call Broker API: GET /api/v1/broker/nd/info 
    No request parameters required.
    """
    endpoint = "https://api-broker.kucoin.com"
    path = "/api/v1/broker/nd/info"
    method = "GET"

    full_path = f"{endpoint}{path}"
    raw_url = path  # No query params, so raw_url is just the path

    # Prepare request
    req = requests.Request(method=method, url=full_path).prepare()

    # No body for GET requests
    process_headers(signer, b"", raw_url, req, method)

    # Send request
    resp = session.send(req)
    print(json.loads(resp.content))


if __name__ == "__main__":
    # Load credentials
    # General API credentials (every user, including Broker Pro or Exchange Broker)
    key = os.getenv("API_KEY", "")
    secret = os.getenv("API_SECRET", "")
    passphrase = os.getenv("API_PASSPHRASE", "")

    # Broker credentials (if applicable)
    brokerName = os.getenv("KC-BROKER-NAME", "")
    brokerPartner = os.getenv("KC-API-PARTNER", "")
    brokerKey = os.getenv("BROKER-KEY", "")

    # Initialize signer and session
    session = requests.Session()
    signer = KcSigner(key, secret, passphrase, brokerPartner, brokerName, brokerKey)

    # Execute General API calls (if general API credentials are provided)
    # get_trade_fees(signer, session)
    # add_limit_order(signer, session)
    # add_futures_limit_order(signer, session)
    
    # Broker Pro calls (if Broker Pro credentials are provided)
    # query_broker_user(signer, session)

    # Exchange Broker calls (if Exchange Broker credentials are provided)
    # query_broker_info(signer, session)
  ```
</Accordion>


<Accordion title="Go">
For a more production-ready implementation, please refer to: [Code](https://github.com/Kucoin/kucoin-universal-sdk/blob/main/sdk/golang/internal/infra/default_signer.go)
    
  ```Go
package main

import (
 "bytes"
 "crypto/hmac"
 "crypto/sha256"
 "encoding/base64"
 "encoding/json"
 "fmt"
 "github.com/google/uuid"
 "io"
 "net/http"
 "os"
 "strconv"
 "time"
)

type KcSigner struct {
 apiKey        string
 apiSecret     string
 apiPassPhrase string
}

func Sign(plain []byte, key []byte) []byte {
 hm := hmac.New(sha256.New, key)
 hm.Write(plain)
 data := hm.Sum(nil)
 return []byte(base64.StdEncoding.EncodeToString(data))
}

func (ks *KcSigner) Headers(plain string) map[string]string {
 t := strconv.FormatInt(time.Now().UnixNano()/1000000, 10)
 p := []byte(t + plain)
 s := string(Sign(p, []byte(ks.apiSecret)))
 ksHeaders := map[string]string{
  "KC-API-KEY":         ks.apiKey,
  "KC-API-PASSPHRASE":  ks.apiPassPhrase,
  "KC-API-TIMESTAMP":   t,
  "KC-API-SIGN":        s,
  "KC-API-KEY-VERSION": "3",
  "X-SITE-TYPE": "global"  /*
  Use "global" by default.
  Set to "australia" when accessing Australia site data or services.
  */
 }
 return ksHeaders
}

func NewKcSigner(key, secret, passPhrase string) *KcSigner {
 ks := &KcSigner{
  apiKey:        key,
  apiSecret:     secret,
  apiPassPhrase: string(Sign([]byte(passPhrase), []byte(secret))),
 }
 return ks
}

func getTradeFees(signer *KcSigner, client *http.Client) {
 endpoint := "https://api.kucoin.com"
 path := "/api/v1/trade-fees"
 method := "GET"
 queryParams := "symbols=BTC-USDT"

 fullURL := fmt.Sprintf("%s%s?%s", endpoint, path, queryParams)
 rawPath := fmt.Sprintf("%s?%s", path, queryParams)

 req, err := http.NewRequest(method, fullURL, nil)
 if err != nil {
  fmt.Println("Error creating request:", err)
  return
 }
 var b bytes.Buffer
 b.WriteString(method)
 b.WriteString(rawPath)
 b.Write([]byte{})

 headers := signer.Headers(b.String())
 for k, v := range headers {
  req.Header.Set(k, v)
 }
 resp, err := client.Do(req)
 if err != nil {
  fmt.Println("Error sending request:", err)
  return
 }
 defer resp.Body.Close()

 body, err := io.ReadAll(resp.Body)
 if err != nil {
  fmt.Println("Error reading response:", err)
  return
 }

 fmt.Println("Response:", string(body))
}

func addLimitOrder(signer *KcSigner, client *http.Client) {
 endpoint := "https://api.kucoin.com"
 path := "/api/v1/hf/orders"
 method := "POST"

 orderData := map[string]interface{}{
  "clientOid": uuid.NewString(),
  "side":      "buy",
  "symbol":    "BTC-USDT",
  "type":      "limit",
  "price":     "10000",
  "size":      "0.001",
 }

 orderDataBytes, err := json.Marshal(orderData)
 if err != nil {
  fmt.Println("Error marshalling order data:", err)
  return
 }

 fullURL := fmt.Sprintf("%s%s", endpoint, path)

 var b bytes.Buffer
 b.WriteString(method)
 b.WriteString(path)
 b.Write(orderDataBytes)

 req, err := http.NewRequest(method, fullURL, bytes.NewBuffer(orderDataBytes))
 if err != nil {
  fmt.Println("Error creating request:", err)
  return
 }

 headers := signer.Headers(b.String())
 for k, v := range headers {
  req.Header.Set(k, v)
 }

 req.Header.Set("Content-Type", "application/json")

 resp, err := client.Do(req)
 if err != nil {
  fmt.Println("Error sending request:", err)
  return
 }
 defer resp.Body.Close()

 body, err := io.ReadAll(resp.Body)
 if err != nil {
  fmt.Println("Error reading response:", err)
  return
 }

 fmt.Println("Response:", string(body))
}

func SignExample() {
 apiKey := os.Getenv("API_KEY")
 apiSecret := os.Getenv("API_SECRET")
 passphrase := os.Getenv("API_PASSPHRASE")

 signer := NewKcSigner(apiKey, apiSecret, passphrase)

 client := &http.Client{}

 getTradeFees(signer, client)
 addLimitOrder(signer, client)
}
  ```
</Accordion>


<Accordion title="PHP">
For a more production-ready implementation, please refer to: [Code](https://github.com/Kucoin/kucoin-universal-sdk/tree/main/sdk/php)
  ```php
<?php

class KcSigner
{
    private $apiKey;
    private $apiSecret;
    private $apiPassphrase;

    public function __construct(string $apiKey, string $apiSecret, string $apiPassphrase)
    {
        $this->apiKey = $apiKey ?: '';
        $this->apiSecret = $apiSecret ?: '';
        $this->apiPassphrase = $apiPassphrase ?: '';

        if ($apiSecret && $apiPassphrase) {
            $this->apiPassphrase = $this->sign($apiPassphrase, $apiSecret);
        }

        if (!$this->apiKey || !$this->apiSecret || !$this->apiPassphrase) {
            fwrite(STDERR, "Warning: API credentials are empty. Public endpoints only.\n");
        }
    }

    private function sign(string $plain, string $key): string
    {
        return base64_encode(hash_hmac('sha256', $plain, $key, true));
    }

    public function headers(string $payload): array
    {
        $timestamp = (string)(int)(microtime(true) * 1000);
        $signature = $this->sign($timestamp . $payload, $this->apiSecret);

        return [
            'KC-API-KEY' => $this->apiKey,
            'KC-API-PASSPHRASE' => $this->apiPassphrase,
            'KC-API-TIMESTAMP' => $timestamp,
            'KC-API-SIGN' => $signature,
            'KC-API-KEY-VERSION' => '3',
            'Content-Type' => 'application/json',
            // Set to "global" for KuCoin Global, or "australia" for KuCoin Australia.
            "X-SITE-TYPE" => "global"
        ];
    }
}

function processHeaders(KcSigner $signer, string $body, string $rawUrl, string $method): array
{
    $payload = $method . $rawUrl . $body;
    return $signer->headers($payload);
}

function httpRequest(string $method, string $url, array $headers, string $body = ''): array
{
    $curl = curl_init();
    if ($curl === false) {
        return ['status' => 0, 'body' => '', 'error' => 'Failed to initialize cURL'];
    }

    $headerLines = [];
    foreach ($headers as $key => $value) {
        $headerLines[] = $key . ': ' . $value;
    }

    curl_setopt($curl, CURLOPT_URL, $url);
    curl_setopt($curl, CURLOPT_RETURNTRANSFER, true);
    curl_setopt($curl, CURLOPT_CUSTOMREQUEST, $method);
    curl_setopt($curl, CURLOPT_HTTPHEADER, $headerLines);
    curl_setopt($curl, CURLOPT_TIMEOUT, 20);
    curl_setopt($curl, CURLOPT_CONNECTTIMEOUT, 10);

    if ($body !== '') {
        curl_setopt($curl, CURLOPT_POSTFIELDS, $body);
    }

    $respBody = curl_exec($curl);
    $curlError = curl_error($curl);
    $status = (int)curl_getinfo($curl, CURLINFO_HTTP_CODE);
    curl_close($curl);

    if ($respBody === false) {
        return ['status' => $status, 'body' => '', 'error' => $curlError];
    }

    return ['status' => $status, 'body' => $respBody, 'error' => ''];
}

function getAccountSummaryInfo(KcSigner $signer): void
{
    $endpoint = 'https://api.kucoin.com';
    $path = '/api/v2/user-info';
    $method = 'GET';
    $rawUrl = $path;
    $url = $endpoint . $rawUrl;

    $headers = processHeaders($signer, '', $rawUrl, $method);

    $resp = httpRequest($method, $url, $headers);
    if ($resp['error'] !== '') {
        echo "Error fetching account summary: " . $resp['error'] . PHP_EOL;
        return;
    }
    if ($resp['status'] >= 400 || $resp['status'] === 0) {
        echo "Error fetching account summary: HTTP " . $resp['status'] . " " . $resp['body'] . PHP_EOL;
        return;
    }

    echo $resp['body'] . PHP_EOL;
}

function main(): void
{
    // Fill in your API credentials here for quick testing.
    // NOTE: Hardcoding secrets in the main program is unsafe. This file is a
    // signing/verification example only and must NOT be used in production.
    $key = '';
    $secret = '';
    $passphrase = '';

    $signer = new KcSigner($key, $secret, $passphrase);

    getAccountSummaryInfo($signer);
}

if (php_sapi_name() === 'cli') {
    main();
}
?>
  ```
</Accordion>
</AccordionGroup>
</Card>
<Card>
## Gateway Timestamp

Gateway Timestamp is a pair of timestamps generated by the Gateway to mark the **inbound** and **outbound** moments of the same request at the Gateway boundary:

| Boundary | Meaning | WebSocket Response (Body) | REST Response (Header) |
|---|---|---|---|
| Inbound | The request arrives at the Gateway | `inTime` | `x-in-time` |
| Outbound | The response is sent from the Gateway | `outTime` | `x-out-time` |

:::tip[Notes]
- The Gateway-side processing duration can be approximated by `outTime - inTime` (WebSocket) or `x-out-time - x-in-time` (REST). The unit depends on the enabled precision.
:::

### Precision Control

Use the following parameters to control Gateway Timestamp precision.

| Protocol | Control Parameter | Parameter Location | Affected Response Location | Affected Fields | Default Unit |
|---|---|---|---|---|---|
| REST | `kc-enable-ns` | Request Header | Response Header | `x-in-time`, `x-out-time` | Microseconds (µs) |
| WebSocket | `enable_ns` | WSS Query Parameter | Response Body | `inTime`, `outTime` | Milliseconds (ms) |

:::tip[Remarks]
1. REST: `x-in-time` and `x-out-time` are returned in the Response Header and may be included in responses for any REST API. The `5-` prefix only indicates that the timestamp pair is generated by the Gateway. Currently, only the Gateway-side inbound and outbound timestamps are provided.
2. WebSocket: `inTime` and `outTime` are in the Response Body and may be returned by request-response Pro WebSocket APIs such as Add/Cancel Order.
3. For Colo customers using UTA accounts, `inTime` and `outTime` are returned in nanoseconds by default, so `enable_ns=true` is not required.
:::
### Enable Nanoseconds

#### RESTful API

Set `kc-enable-ns: true` in the request header to return `x-in-time` and `x-out-time` in nanoseconds.

Request Example:
```python
import requests

url = "https://api.kucoin.com/api/ua/v1/market/currency"
params = {"currency": "USDT"}

headers = {
    "kc-enable-ns": "true",              // Enable Nanoseconds
    "KC-API-KEY": "<REDACTED>",
    "KC-API-SIGN": "<REDACTED>",
    "KC-API-TIMESTAMP": "1768982284539",
    "KC-API-PASSPHRASE": "<REDACTED>",
    "KC-API-KEY-VERSION": "3",
    "Content-Type": "application/json",
    "User-Agent": "Kucoin-Universal-Postman-SDK/v1.3.0",
    "Accept": "*/*",
    "Accept-Encoding": "gzip, deflate, br",
    "Connection": "keep-alive",
    "Host": "api.kucoin.com",
}

resp = requests.get(url, params=params, headers=headers, timeout=10)

print("x-in-time:", resp.headers.get("x-in-time"))
print("x-out-time:", resp.headers.get("x-out-time"))
print(resp.status_code)
print(resp.text)
```

Response Example (Response Header):
```http
Date: Wed, 21 Jan 2026 07:58:07 GMT
Content-Type: application/json
Transfer-Encoding: chunked
Connection: keep-alive
CF-RAY: 9c1543be6a0aff91-SIN
x-in-time: 5-1768982287151457725         // Inbound Timestamp
gw-ratelimit-remaining: 1997
gw-ratelimit-limit: 2000
gw-ratelimit-reset: 29994
x-out-time: 5-1768982287162161420        // Outbound Timestamp
X-RESULT-BIZ-CODE: 200000
Strict-Transport-Security: max-age=9800; includeSubDomains; preload
Vary: Origin
vary: accept-encoding
Content-Encoding: gzip
cf-cache-status: DYNAMIC
Set-Cookie: <REDACTED>
Server: cloudflare
```

#### WebSocket Add/Cancel Order API

Add `enable_ns=true` to the WebSocket connection URL to return `inTime` and `outTime` in nanoseconds.

Request Example:
```text
wss://wsapi.kucoin.com/v1/private?apikey=689***2b6&timestamp=1768878266952&sign=H/G***%3D&passphrase=aLs***3D&enable_ns=true
```

Response Example (Response Body):
```json
{
    "code": "200000",
    "data": {
        "clientOid": "32dd404b1d724df59ee6f2bd8e8909f60522e89f",
        "orderId": "403363205159706624",
        "tradeType": "FUTURES",
        "ts": 1768982484257000000
    },
    "id": "1b7ac024f4ee42e48d41104c10a21732",
    "inTime": 1768982484219053000,       // Inbound Timestamp
    "op": "uta.order",
    "outTime": 1768982484257824000,      // Outbound Timestamp
    "userRateLimit": {
        "limit": 2000,
        "remaining": 1999,
        "reset": 30000
    }
}
```
</Card>
