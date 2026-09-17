+++
title = "Protecting an Application"
description = "IP filter, authentication, rate limit and audit log in one setup"
weight = 5
+++

# Protecting an Application

This guide puts an internal application behind r3v3rs3 and lets in only the people who should reach it. It uses four mechanisms together:

| Mechanism | Answer to a client that fails it |
|---|---|
| IP filter | `403 Forbidden` |
| Rate limit | `429 Too Many Requests` with `Retry-After` |
| Authentication | `401 Unauthorized` |
| Audit log | Records who changed the configuration |

r3v3rs3 checks them in this order: IP filter, rate limit, HTTPS redirect, redirect rules, authentication. A blocked address never reaches the password check, and a client over the limit never reaches the upstream server.

Follow [Getting Started](@/tutorials/getting-started.md) first. This guide continues from a working port and a working application.

## Step 1: Create an Access List

An access list holds an IP filter and an authentication under one name. Several proxies and routes use the same list, and one change applies to all of them.

1. Click **Access Lists** in the menu, then **Add**.
2. Write `Office` in the name field.
3. Write your office network in **Allowed IP Addresses**, for example `203.0.113.0/24`. Only these clients reach the proxy. Leave it empty to allow every address.
4. Select **Basic Auth** in the authentication section.
5. Write `Staff` in **Realm**. The browser shows this name in its sign-in dialog.
6. Click **Add User**, then write a username and a password.
7. Save.

r3v3rs3 stores each password as an argon2 hash. `access_lists.toml` holds the list with the mode `0600`, and the admin API returns `password_set: true` instead of the hash.

Use **Denied IP Addresses** for a single address that must not reach the proxy. A denied address wins over an allowed address.

## Step 2: Attach the List to the Proxy

1. Open the proxy and find the **Access List** field.
2. Select `Office`.
3. Save.

The list replaces the **IP Filter** and the **Authentication** of the proxy. A proxy cannot hold both, and the admin API answers `400 access_list_conflict` when it does.

Check the result:

```bash
$ curl -i https://app.example.com/
HTTP/2 401
www-authenticate: Basic realm="Staff", charset="UTF-8"

$ curl -i -u alice:<password> https://app.example.com/
HTTP/2 200
```

A client outside the allowed addresses gets `403 Forbidden` and no sign-in dialog.

## Step 3: Add a Rate Limit

The rate limit stays on the proxy, because an access list holds no limit.

1. Open the proxy and find the **Rate Limit** section.
2. Write `60` in **Requests** and select `minute` in **Per**.
3. Write `10` in **Burst**, so a page that loads ten files at once passes.
4. Save.

```bash
$ curl -i -u alice:<password> https://app.example.com/
HTTP/2 429
retry-after: 14
```

- r3v3rs3 counts each client IP address on its own.
- A limit protects the password check too: argon2 takes CPU time on purpose, so a limit slows down password guessing.
- The counters live in memory. A restart resets them. In a cluster the nodes share the counts, see [Rate Limit Accuracy](@/cluster.md#rate-limit-accuracy).

A route can hold its own limit. Give the sign-in path of your application a tighter one:

1. Open the route and turn on **Override Rate Limit for This Route**.
2. Write `5` in **Requests** and select `minute`.

## Step 4: Choose the Authentication

Basic Auth needs no other service, but it shows a browser dialog and has no sign-out. Three other methods fit other cases:

| Method | Use it when |
|---|---|
| **Basic Auth** | A few people share a password, and a browser dialog is enough. |
| **Bearer Token** | A script or a CI job calls an API. Create the token with `openssl rand -hex 32`. |
| **Admin Session** | Your application has no sign-in of its own, and the people already have an r3v3rs3 panel account. |
| **Forward Auth** | An external service decides, for example oauth2-proxy or Authelia with an identity provider. |

**Admin Session** serves a sign-in page at `/.r3v3rs3/auth/login` below the route path. It accepts only an account that sees the proxy, and it asks for the TOTP code of an account with TOTP. Add a sign-out button to your application:

```html
<form method="post" action="/.r3v3rs3/auth/logout"><button>Sign Out</button></form>
```

[Authentication](@/configuration.md#authentication) describes each method with its fields and its responses.

## Step 5: Open One Path to Everybody

A health check or a webhook needs no credentials. Give it its own route:

1. Open the proxy and add a route with the path `/healthz`.
2. Turn on **Override Authentication for This Route** and select **None**.
3. Turn on **Override IP Filter for This Route** and leave both lists empty. The route then allows every client.

The order of the routes does not matter. r3v3rs3 sends each request to the route with the longest matching path, so `/healthz` wins over `/` for that one path.

## Step 6: Get the Real Client IP

Behind a CDN or a load balancer, the TCP peer is the edge server. Without the right setting the IP filter and the rate limit see one address for every visitor.

- r3v3rs3 trusts the IP ranges of eight known CDNs by default, among them Cloudflare, Fastly and Amazon CloudFront. It reads the provider header, for example `CF-Connecting-IP`.
- For your own load balancer, write its address in **Trusted Proxies** of the proxy.
- For an untrusted peer, r3v3rs3 removes `X-Forwarded-For`, `X-Real-IP` and the other client IP headers, because a client can forge them.

Check the resolved address in the access log of your application. r3v3rs3 sends it in `X-Real-IP`. [Client IP](@/configuration.md#client-ip) describes the order of the headers.

## Step 7: Read the Audit Log

The audit log records every configuration change of an account, and every sign-in of the admin panel. It does not record the requests of your visitors.

1. Click **Audit Log** in the menu. Only an admin account opens the page.
2. Filter by account, by resource or by period.

```bash
$ curl -b session.txt 'http://127.0.0.1:46492/api/audit?limit=5'
[{"time":1789643174264,"username":"admin","client":"127.0.0.1","action":"update_access_list","resource_id":"tkx-xqy","summary":"Office"}]
```

Each entry holds the time, the account, the client IP address, the action, the id of the resource and a short summary. A summary never holds a password, a token or a key. The **Audit Log Retention** setting keeps an entry for one year by default.

## What This Does Not Do

- The password check protects the proxy, not the application. A client that reaches the application on another port skips r3v3rs3. Bind the application to `127.0.0.1` or to an internal network.
- r3v3rs3 removes the `Authorization` header before it sends the request to the upstream server, so the application does not see the proxy credentials.
- The rate limit counts requests, not bytes. Use **Request Body Limit** for an upload limit. A request over that limit gets `413 Payload Too Large` before authentication.

## Next Steps

- [Access Lists](@/configuration.md#access-lists): the list model and its errors.
- [IP Filter](@/configuration.md#ip-filter) and [Rate Limit](@/configuration.md#rate-limit): every field.
- [Accounts](@/accounts.md): the roles, the proxy lists and the TOTP accounts.
