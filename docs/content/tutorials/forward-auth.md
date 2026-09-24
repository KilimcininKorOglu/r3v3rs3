+++
title = "Single Sign-On with Forward Auth"
description = "Put oauth2-proxy in front of an application and pass the user to the upstream server"
weight = 15
+++

# Single Sign-On with Forward Auth

Forward auth asks an external service whether to allow each request. The service holds the identity provider session; r3v3rs3 only asks it and copies the answer. This is the `auth_request` of nginx and the ForwardAuth middleware of Traefik.

Use it when you already run oauth2-proxy, Authelia or a service of your own. For accounts that r3v3rs3 itself keeps, [Accounts for a Team](@/tutorials/team-accounts.md) is simpler.

## Step 1: Run the Auth Service

oauth2-proxy with a Google client, as an example:

```yaml
services:
  oauth2-proxy:
    image: quay.io/oauth2-proxy/oauth2-proxy:v7.6.0
    command:
      - --http-address=0.0.0.0:4180
      - --provider=google
      - --email-domain=example.com
      - --upstream=static://202
      - --reverse-proxy=true
      - --cookie-secure=true
      - --set-xauthrequest=true
    environment:
      OAUTH2_PROXY_CLIENT_ID: <client id>
      OAUTH2_PROXY_CLIENT_SECRET: <client secret>
      OAUTH2_PROXY_COOKIE_SECRET: <32 byte secret>
    ports:
      - 127.0.0.1:4180:4180
```

Two options matter for r3v3rs3:

- `--reverse-proxy=true` makes oauth2-proxy read the `X-Forwarded-*` headers that r3v3rs3 sends.
- `--set-xauthrequest=true` puts the user in the `X-Auth-Request-User` and `X-Auth-Request-Email` response headers, which r3v3rs3 can copy.

`/oauth2/auth` is the endpoint that answers 202 for a signed-in client and 401 for everyone else. `/oauth2/start` and `/oauth2/callback` serve the browser sign-in.

## Step 2: Publish the Auth Endpoints

The browser must reach `/oauth2/` on the same host as the application, because the session cookie belongs to that host. Add a route to your proxy:

| Route | Servers | Authentication |
|---|---|---|
| `/oauth2` | `http://127.0.0.1:4180/oauth2/` | **None** |
| `/` | your application | **Forward Auth** |

The longest matching path wins, so `/oauth2/start` goes to oauth2-proxy and everything else goes to the application. r3v3rs3 removes the route path by default (**Remove Route Path**), so the server URL of the `/oauth2` route ends with `/oauth2/`. Turn on **Override Authentication for This Route** on the `/oauth2` route and select **None**; without that, a client would need a session to reach the sign-in page.

## Step 3: Turn On Forward Auth

Set the authentication of the proxy to **Forward Auth**:

| Field | Value |
|---|---|
| **Auth URL** | `http://127.0.0.1:4180/oauth2/auth` |
| **Copy Response Headers** | `X-Auth-Request-User`, `X-Auth-Request-Email` |
| **Timeout (Seconds)** | `10` |

In `proxies.toml`:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [
  { path = "/oauth2", servers = [{ url = "http://127.0.0.1:4180/oauth2/" }], auth = { type = "none" } },
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
]
```

## Step 4: What r3v3rs3 Sends

For each client request, r3v3rs3 sends a `GET` to the auth URL. The auth request carries the client request headers, without the connection headers and `Host`, plus five headers:

```
x-forwarded-method: GET
x-forwarded-proto: http
x-forwarded-host: app.example.com
x-forwarded-uri: /
x-forwarded-for: 203.0.113.7
```

`X-Forwarded-For` holds the client IP address that the [Client IP](@/configuration.md#client-ip) settings resolve, so a CDN in front does not hide the real client from your policy.

The auth request trusts the same root certificates as the upstream requests, and sends the client certificate of the proxy. So an auth service over HTTPS with mutual TLS works without a second certificate setting.

## Step 5: What r3v3rs3 Does With the Answer

| Auth response | Result |
|---|---|
| 2xx | The request goes to the upstream server with the copied headers. |
| Any other status | The client receives the auth response: status, headers and a body up to 64 KiB. |
| No response within the timeout, or a connection error | The client receives 502 Bad Gateway. |

The second row is what makes the browser sign-in work. An auth service that answers `302 Found` with a `Location` sends the browser to the login page, and r3v3rs3 passes that redirect through:

```bash
$ curl -i https://app.example.com/
HTTP/1.1 302 Found
location: https://sso.example.com/login
```

oauth2-proxy answers `/oauth2/auth` with 401 and no redirect, so r3v3rs3 passes the 401 to the browser. Send the user to `/oauth2/start?rd=/` from the error page or a link of your application; oauth2-proxy then starts the sign-in.

With a session, the same request reaches the application:

```bash
$ curl -s https://app.example.com/ -H 'Cookie: <session>'
200
```

When the auth service is down, every request answers 502. Run it next to r3v3rs3, or give the proxy a route that skips it for the health check of your load balancer.

## Step 6: See the User in the Upstream Request

The headers in **Copy Response Headers** are copied from the auth response to the upstream request:

```json
{
  "host": "127.0.0.1:9000",
  "cookie": "sso=ok",
  "x-auth-request-user": "alice@example.com",
  "x-auth-request-email": "alice@example.com",
  "x-forwarded-for": "203.0.113.7",
  "x-forwarded-proto": "http",
  "x-forwarded-host": "app.example.com",
  "via": "r3v3rs3"
}
```

Your application reads `X-Auth-Request-User` and knows who is calling, without an identity library.

**A client cannot forge these headers.** r3v3rs3 removes every header of the list from the client request before it copies the auth response. A request that carries `X-Auth-Request-User: attacker@evil` reaches the upstream server with the value from the auth service:

```
x-auth-request-user: alice@example.com
```

Note that the auth request itself still carries the client headers, so your auth service sees the forged value. Ignore the list headers there.

## Step 7: Leave a Route Open

A health check or a webhook must not go through the sign-in. Give it its own route, turn on **Override Authentication for This Route** and select **None**:

```toml
{ path = "/healthz", servers = [{ url = "http://127.0.0.1:9000/healthz" }], auth = { type = "none" } }
```

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' https://app.example.com/healthz
200
```

A route replaces the authentication of the proxy, so this also works the other way: one route with forward auth on a proxy that is otherwise open.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Every request answers 502 | The auth service does not answer, or the URL is wrong. |
| The browser loops between the application and the login page | The `/oauth2` route is missing or it is not set to **None**. |
| The upstream server receives no user header | The header is not in **Copy Response Headers**, or oauth2-proxy runs without `--set-xauthrequest`. |
| The auth service sees `127.0.0.1` as the client | The [Client IP](@/configuration.md#client-ip) settings do not name your CDN or trusted proxy. |

## Reference

- [Forward Auth](@/configuration.md#forward-auth): the headers, the response rules and the timeout.
- [Client IP](@/configuration.md#client-ip): how `X-Forwarded-For` is resolved.
- [Routing](@/configuration.md#routing): why the longest path wins.

## Next Steps

- [Accounts for a Team](@/tutorials/team-accounts.md): the same protection with r3v3rs3 accounts.
- [Protecting an Application](@/tutorials/protect-an-app.md): IP filter, rate limit and audit log.
