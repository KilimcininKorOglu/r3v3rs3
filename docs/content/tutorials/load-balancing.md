+++
title = "Load Balancing and Health Checks"
description = "Spread the traffic over several application servers and take a failing one out"
weight = 11
+++

# Load Balancing and Health Checks

This guide puts three copies of one application behind one proxy. You give each copy a share of the traffic, let r3v3rs3 find the failing copy, keep each client on one server, and take a server out of service without a restart.

You need a port and a proxy from [Getting Started](@/tutorials/getting-started.md).

## Step 1: Add the Servers

Open your HTTP proxy and write the servers of the route in **Target**, one per line:

```
http://10.0.0.1:9000/
http://10.0.0.2:9000/
http://10.0.0.3:9000/
```

**Load Balancing** selects the policy of the proxy:

| Policy | What it does |
|---|---|
| Round robin | Uses the servers in turn. The default. |
| Random | Selects a random server. |
| First healthy server | Uses the first server, the others are backups. |
| Client IP hash | Sends each client IP address to the same server. |

An HTTP proxy selects a server for each request, a TCP proxy for each connection, and a UDP proxy for each client session. The servers of each HTTP route are a separate group, so two routes of one proxy can use different server sets.

## Step 2: Give a Server More Traffic

The weight of a server is a whole number from 0 to 65535, and the default is 1. An HTTP proxy takes it after the URL in **Target**, for example `http://10.0.0.3:9000/ 3`. A TCP or UDP proxy has a **Weight** field for each server. Round robin uses each server as often as its weight, and it spreads the turns over the cycle instead of sending a burst.

Two servers with the weights 1 and 3 answer in this order:

```
server-2 server-1 server-2 server-2 server-2 server-1 server-2 server-2
```

Three of every four requests go to the server with the weight 3. Use this when one machine is larger than the other.

Random selects a server with a chance that is proportional to its weight. Client IP hash gives each server a share of the client addresses that is proportional to its weight. First healthy server ignores the weight.

## Step 3: Find the Failing Server

Without an active health check, r3v3rs3 learns about a failure only from the traffic. **Max Fails** consecutive failures (default 1) mark a server unhealthy for **Fail Timeout (Seconds)** (default 30 seconds). A failure is a failed connection or a request without a response. A 500 response is a success, because the server answered.

Turn on the active check to find a server that is up but broken:

1. Set **Check Interval (Seconds)** to `2`.
2. Set **Health Check Path** to `/health`.
3. Leave **Check Timeout (Seconds)** at `5`.

Every interval r3v3rs3 sends `GET /health` to each server. A 2xx or 3xx status passes. Without a path, an HTTP proxy and a TCP proxy open a TCP connection, and a UDP proxy resolves the host name.

Ask the status API for the result:

```bash
$ curl -s -b cookies.txt http://127.0.0.1:46492/api/proxies/<proxy-id>/status
```

A server whose `/health` answers 500 appears as:

```json
{
  "addr": "http://10.0.0.3:9000/",
  "weight": 1,
  "healthy": false,
  "failures": 0,
  "last_error": "the active check received status 500 Internal Server Error"
}
```

From that moment every request goes to the two healthy servers. The server returns when a check passes. A successful request does not end this state; only a passing check does.

The proxy list of the WebUI shows `2/3 healthy` and names the unhealthy server in the title of the number. It refreshes every 10 seconds.

When every server is unhealthy, r3v3rs3 still sends the traffic to them. It does not answer 503 on its own.

## Step 4: Retry a Failed Request

**Attempts** (default 2) is the number of tries of one request, including the first one. **Retry On** lists the failures that start a retry:

- Failed connection: the connection fails or the connect timeout expires. Every method is retried, because the server received nothing.
- Request timeout, 502, 503, 504: only the idempotent methods `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT` and `DELETE` are retried.

A retry goes to the next server and skips a server whose circuit is open. With three attempts and one dead server in the list, every client still receives 200 while the status API counts the failures of that server:

```json
{ "addr": "http://10.0.0.9:9000/", "healthy": false, "failures": 1, "last_error": "client error (Connect)" }
```

A request with a body is retried only when the body length is known and is at most **Replay Body Limit (Bytes)** (default 0). r3v3rs3 keeps such a body in memory. A WebSocket or other upgrade request is never retried. A route can use its own retry settings with **Override Retries for This Route**.

## Step 5: Keep a Client on One Server

An application that keeps a session in the memory of one server needs sticky sessions.

1. Turn on **Enable Sticky Cookie** on the route.
2. Set **Cookie Name** to `app_server`.
3. Set **Cookie Max-Age (Seconds)** to `3600`.

The first response sets the cookie:

```
set-cookie: app_server=1f8a...; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600
```

Every next request with that cookie goes to the same server. The value is an HMAC signature of the proxy, the route and the server URL, so a client cannot select a server with a forged value. r3v3rs3 removes the cookie from the request, so the upstream server never sees it.

The client moves to another server when its server is unhealthy or its circuit is open. r3v3rs3 creates a new signature key at each start, so a restart invalidates every cookie. Set Max-Age to 0 for a cookie that ends with the browser session.

A TCP or UDP proxy has no cookies. Use **Client IP hash** there.

## Step 6: Take a Server Out of Service

Set the weight of the server to `0` and click **Update**: write `http://10.0.0.3:9000/ 0` in **Target**, or `0` in the **Weight** field of a TCP or UDP proxy. The server gets no new traffic, also when every other server is unhealthy. Its open TCP connections and UDP sessions stay, and its sticky clients stay, so the sessions on it end on their own.

At least one server must have a weight above 0. r3v3rs3 refuses a route where every server is drained.

This is the way to deploy: drain one server, wait for its sessions, upgrade it, set the weight back.

## Step 7: Stop the Traffic to a Broken Server

The circuit breaker is off by default. Turn on **Enable Circuit Breaker** on the proxy:

| Field | Default | Meaning |
|---|---|---|
| **Failure Ratio (%)** | 50 | The share of failed requests that opens the circuit. |
| **Minimum Requests** | 20 | The window needs this many requests before the ratio counts. |
| **Window (Seconds)** | 10 | The length of one counting window. |
| **Open Duration (Seconds)** | 30 | The time without traffic after the circuit opens. |

A request fails when the connection fails, a timeout expires, or the server answers 502, 503 or 504. After Open Duration one trial request tests the server: a success closes the circuit, a failure opens it again. When every server of a route has an open circuit, the client receives 503 Service Unavailable without a request to a server.

The status API shows `"circuit": "open"` or `"circuit": "half_open"`. UDP proxies have no circuit breaker.

## Step 8: Send a Copy of the Traffic to a New Version

**Mirror the Requests of This Route** copies the requests of the route to other servers, so you can test a new version with real traffic:

1. Add the new version to **Mirror Servers**.
2. Set **Mirrored Requests (%)** to `10`.

Every mirror server receives a copy of the mirrored requests with the same method, headers, path and body. r3v3rs3 drops their responses, does not retry them, does not wait for them and does not count them in the health checks. The client receives the response of the route server as before.

A copy goes out after r3v3rs3 read the whole request body. A cached response, an upgrade request, a body over the maximum body size (default 65536 bytes) and a request that arrives while 64 copies are in progress are not copied. A copy carries the credentials that authentication does not remove, such as cookies, so mirror only to a server that you trust with that data.

## WebSocket and gRPC

Both work over the same HTTP proxy, but they need different settings.

### WebSocket

A WebSocket proxy needs no option. r3v3rs3 passes the upgrade and the frames of an HTTP or HTTPS proxy in both directions:

```
ws://app.example.com/socket  -> a route with http://10.0.0.1:9000/
wss://app.example.com/socket -> the same route on an HTTPS port
```

Two things change for a WebSocket connection:

- It is never retried and never mirrored, because the body does not end.
- It always uses HTTP/1.1 toward the upstream server, also with h2c on.

Set the request timeout of the route to `0` when your connections stay open longer than the timeout.

### gRPC

gRPC needs HTTP/2 end to end.

On an HTTPS port the clients get HTTP/2 from ALPN. Toward the upstream server:

- An HTTPS server negotiates `h2` with ALPN on its own, so nothing is needed.
- A plain HTTP server receives HTTP/1.1, which gRPC cannot use. Turn on **Use HTTP/2 for Plain HTTP Servers (h2c)** on the proxy.

Turn h2c on only when every plain HTTP server of the proxy accepts HTTP/2 with prior knowledge. A server that does not answers with an error on the first request.

The proxies of a port that use the same client certificate and the same connect timeout share the upstream connections, so one HTTP/2 connection carries the streams of many clients.

## Reference

- [Load Balancing and Health Checks](@/configuration.md#load-balancing-and-health-checks): every field and its default.
- [Circuit Breaker](@/configuration.md#circuit-breaker) and [Sticky Sessions](@/configuration.md#sticky-sessions).
- [HTTP/2](@/configuration.md#http-2), [WebSocket](@/configuration.md#websocket) and [Traffic Mirroring](@/configuration.md#traffic-mirroring).

## Next Steps

- [Protecting an Application](@/tutorials/protect-an-app.md): access list, authentication and rate limit.
- [Caching and Compression](@/tutorials/cache-and-compression.md): fewer requests to the servers.
- [High Availability](@/tutorials/high-availability.md): several r3v3rs3 nodes with one shared state.
