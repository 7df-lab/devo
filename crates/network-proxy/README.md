# devo-network-proxy

`devo-network-proxy` provides small reqwest helpers for Devo local tools.

The helper applies proxy settings in this order:

1. The explicit config proxy URL passed by the caller.
2. `http_proxy` / `HTTP_PROXY` for HTTP requests.
3. `https_proxy` / `HTTPS_PROXY` for HTTPS requests.
4. `all_proxy` / `ALL_PROXY` as a fallback.

Config proxy URLs are applied with `reqwest::Proxy::all`, so a configured
`http://`, `socks5://`, or `socks5h://` proxy is used for all local tool HTTP
requests. Environment values are only used when no config proxy URL is present.
