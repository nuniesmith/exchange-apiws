exchange-apiws

contract_value hardcoded table — replace the match + silent _ => 1.0 fallback with a runtime fetch from get_contract() and a proper error if the multiplier is missing. This is a library correctness issue — any caller using calc_contracts on an unlisted symbol gets silently wrong sizing.
reqwest version — bump from 0.12 to 0.13 to match what kucoin-futures already pins, so Cargo doesn't compile two copies.
No integration tests — the only tests are unit-level (auth signing, candle parsing). Add a mockito/wiremock harness that exercises at least get_balance, place_order, envelope unwrapping, and a WS frame parse round-trip. Catches regressions before they touch live funds.
