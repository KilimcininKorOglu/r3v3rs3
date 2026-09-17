+++
title = "Migrating from nginx or Traefik"
description = "Translate your configuration, run both proxies side by side and cut over"
weight = 16
+++

# Migrating from nginx or Traefik

This guide translates the parts of an nginx or Traefik configuration that a reverse proxy usually holds, then cuts the traffic over without a window of downtime.

The plan: run r3v3rs3 on another port, rebuild the configuration there, test it with a `Host` header, then move the port.

## Step 1: List What You Have

Write down every item of your current configuration. Most setups have five kinds:

1. The server blocks or routers: which host goes where.
2. The locations or rules: which path goes where, and with which path rewrite.
3. The certificates: files, or ACME.
4. The extras: redirects, headers, basic auth, rate limits, IP filters, caching.
5. The upstreams: the servers, their weights and their health checks.

Everything in the tables below has an r3v3rs3 equivalent. Anything that is not there, for example a Lua script or an nginx module, does not, and that part needs another answer.

## Step 2: Translate the nginx Directives

| nginx | r3v3rs3 |
|---|---|
| `listen 443 ssl;` | A port with the protocol **HTTPS** and **TLS Termination** |
| `listen 443 quic;` | A port with the protocol **HTTP over QUIC (HTTP/3)** |
| `server_name app.example.com;` | **Virtual Hosts** of the proxy |
| `location /api { }` | A route with the path `/api` |
| `proxy_pass http://api:8080/v1/;` | A server `http://api:8080/v1/` on the route |
| `upstream app { server a; server b; }` | Two servers on one route |
| `server a weight=3;` | **Weight** `3` on the server |
| `return 301 https://$host$request_uri;` | **Automatically Redirect HTTP to HTTPS** |
| `rewrite ^/items/([0-9]+)$ /item/$1 break;` | **Rewrite** with `regex` and `replacement` on the route |
| `add_header X-Frame-Options DENY;` | A response header rule: `set X-Frame-Options: DENY` |
| `auth_basic` and `auth_basic_user_file` | **Basic Auth** with users on the proxy |
| `auth_request /auth;` | **Forward Auth** with the auth URL |
| `allow` and `deny` | **IP Filter** allow and deny lists |
| `limit_req_zone` and `limit_req` | **Rate Limit** with requests, period and burst |
| `proxy_cache_path` and `proxy_cache` | **Cache** with a memory limit and a default TTL |
| `gzip on;` | **Compression** with the algorithms and a minimum size |
| `return 404;` | A route with the fixed response **Fixed status** |
| `proxy_set_header X-Real-IP $remote_addr;` | Automatic. See [Client IP](@/configuration.md#client-ip) |
| `client_max_body_size 10m;` | **Request Body Limit** |
| `proxy_read_timeout 60s;` | **Request Timeout** of the proxy or the route |
| `stream { server { proxy_pass db:5432; } }` | A TCP proxy |

Two differences that cost people time:

- **The order of the locations does not matter.** nginx picks by its own precedence rules and the order in the file. r3v3rs3 always sends the request to the route with the longest matching path, over every proxy of the port. `/api` matches `/api` and `/api/users`, never `/apiv2`.
- **The trailing slash rule is not the same.** In nginx, `proxy_pass http://api:8080/` strips the location prefix and `proxy_pass http://api:8080` keeps it. In r3v3rs3 the route path is always removed and the rest is added to the path of the server URL:

```
route /api, server http://api:8080/v1/   ->  GET /api/users  =  /v1/users
route /keep, strip_prefix = false        ->  GET /keep/x     =  /keep/x
```

The query string is kept in both cases.

## Step 3: Translate the Traefik Labels

| Traefik | r3v3rs3 |
|---|---|
| `entrypoints` | Ports |
| `Host(\`app.example.com\`)` | **Virtual Hosts** of the proxy |
| `PathPrefix(\`/api\`)` | A route with the path `/api` |
| A router without `PathPrefix` | A route with the path `/` |
| `service` with `loadBalancer.servers` | The servers of the route |
| `loadBalancer.healthCheck` | **Health Check** with a path and an interval |
| `loadBalancer.sticky.cookie` | **Enable Sticky Cookie** with a cookie name |
| `middlewares.stripPrefix` | The default behavior of a route |
| `middlewares.addPrefix` | **Rewrite** with `add_prefix` |
| `middlewares.redirectScheme` | **Automatically Redirect HTTP to HTTPS** |
| `middlewares.redirectRegex` | A redirect rule with `regex`, `target` and `status` |
| `middlewares.basicAuth` | **Basic Auth** |
| `middlewares.forwardAuth` | **Forward Auth**. See [Single Sign-On with Forward Auth](@/tutorials/forward-auth.md) |
| `middlewares.ipAllowList` | **IP Filter** |
| `middlewares.rateLimit` | **Rate Limit** |
| `middlewares.headers` | Header rules |
| `middlewares.compress` | **Compression** |
| `certresolver` with ACME | An ACME entry. See [HTTPS with a Wildcard Certificate](@/tutorials/https-certificates.md) |
| Docker labels | Docker service discovery. See [Proxies from Docker](@/tutorials/docker-discovery.md) |
| A `tcp` router | A TCP proxy |

A Traefik setup that is built from Docker labels needs the least work: turn on the Docker provider and write the `r3v3rs3.` labels next to the `traefik.` ones. Both proxies then read the same containers, and you can compare them before you remove anything.

## Step 4: Build It on Another Port

Do not touch port 80 and 443 yet. Give r3v3rs3 a free port:

1. Add a port on `0.0.0.0:8080` with the protocol **HTTP**.
2. Build every proxy, with the real virtual hosts.
3. Test each one with a `Host` header:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: app.example.com' http://127.0.0.1:8080/
200
$ curl -s -H 'Host: app.example.com' http://127.0.0.1:8080/api/users
```

Nothing in DNS changes, and your users stay on the old proxy. Work through the list of Step 1 until every item answers as before.

For HTTPS, add a port on `0.0.0.0:8443` and use `--resolve`:

```bash
$ curl -sI --resolve app.example.com:8443:127.0.0.1 https://app.example.com:8443/
```

## Step 5: Compare the Responses

The status is not enough. Compare the headers of both proxies:

```bash
$ curl -sI -H 'Host: app.example.com' http://127.0.0.1:8080/ > new.txt
$ curl -sI https://app.example.com/ > old.txt
$ diff old.txt new.txt
```

Look at the cache headers, the `Set-Cookie` attributes, the CORS headers and the redirects. r3v3rs3 adds `via: r3v3rs3` and the `forwarded` and `x-forwarded-*` headers to the upstream request:

```json
{
  "forwarded": "for=203.0.113.7, host=app.example.com, proto=https",
  "x-forwarded-for": "203.0.113.7",
  "x-forwarded-proto": "https",
  "x-forwarded-host": "app.example.com",
  "via": "r3v3rs3"
}
```

An application that reads `X-Real-IP` needs a request header rule that sets it, because r3v3rs3 does not send that header.

If you sit behind a CDN, set the CDN in [Client IP](@/configuration.md#client-ip) before you compare. Otherwise every log line holds the address of the CDN.

## Step 6: Cut Over

Do it in this order:

1. Lower the TTL of the DNS records to five minutes, one day in advance. This applies only when you move to another machine.
2. Stop the old proxy, or move it to another port. Two processes cannot listen on the same port.
3. Change the r3v3rs3 ports from `8080` and `8443` to `80` and `443`. The change applies without a restart.
4. Check the certificates. With ACME, the HTTP-01 challenge needs port 80 and the TLS-ALPN-01 challenge needs port 443, so an order before the cutover can fail.
5. Test the real address:

```bash
$ curl -sI https://app.example.com/
```

6. Watch the proxy list for a few minutes. The number of healthy servers and the issues of the discovery providers appear there.

To go back, change the ports back and start the old proxy. Keep its configuration file until you are sure, because the path back is exactly this.

## Step 7: Remove the Old Setup

After a few days of quiet:

- Remove the old proxy and its configuration.
- Remove its certificate files, unless r3v3rs3 uses them.
- Remove the `traefik.` labels from your containers, when you moved to Docker service discovery.
- Delete the `8080` and `8443` ports from r3v3rs3.

## What Does Not Have an Equivalent

| Feature | Status |
|---|---|
| Lua, njs and other nginx modules | Not available. |
| An exact path match | Not available. A path matches whole segments as a prefix. |
| A response body rewrite (`sub_filter`) | Not available. |
| A disk cache | Not available. The cache is in memory. |
| HTTP/3 to the upstream server | Not available. The upstream connection uses HTTP/2 or HTTP/1.1. |
| WebTransport | Not available. |

## Reference

- [Routing](@/configuration.md#routing) and [Path Rewrite](@/configuration.md#path-rewrite).
- [Redirect Rules](@/configuration.md#redirect-rules) and [Fixed Responses](@/configuration.md#fixed-responses).
- [Client IP](@/configuration.md#client-ip): the headers and the trusted proxies.

## Next Steps

- [Load Balancing and Health Checks](@/tutorials/load-balancing.md): the upstream part of the migration.
- [HTTPS with a Wildcard Certificate](@/tutorials/https-certificates.md): the certificates.
- [Proxies from Docker](@/tutorials/docker-discovery.md): labels instead of a configuration file.
