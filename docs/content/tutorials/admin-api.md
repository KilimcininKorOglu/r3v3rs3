+++
title = "Driving r3v3rs3 from a Script"
description = "Sign in to the admin API, create a port and a proxy, and purge the cache from CI"
weight = 12
+++

# Driving r3v3rs3 from a Script

The WebUI has no button that the WebUI itself does not use. Everything it does goes through the admin API under `/api`, so a script can do the same. This guide signs in, creates a port and a proxy, reads the status, purges the cache from a deploy job and gives the job an account that cannot do anything else.

The examples use `curl` against `http://localhost:46492`, the default address of the admin panel.

## Step 1: Sign In

`POST /api/login` returns a session cookie:

```bash
$ curl -s -c cookies.txt \
    -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd","insecure":true}' \
    http://localhost:46492/api/login
"success"
```

The response carries the cookie:

```
set-cookie: token=QWZ1uBQz5A9gAnmK2b18EZ7tD7CpkJ9I; HttpOnly; SameSite=Strict
```

`"insecure": true` removes the `Secure` attribute, so the cookie also works over plain HTTP. Drop it when the admin panel runs behind HTTPS.

Every other endpoint needs that cookie:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/ports
[{"id":"dpv-ylj","name":"web","listen":"/ip4/127.0.0.1/tcp/8473/http"}]
$ curl -s -o /dev/null -w '%{http_code}\n' http://localhost:46492/api/ports
401
```

A wrong password answers 400:

```json
{"message":"invalid login credentials","error":{"message":"invalid_login_credentials"}}
```

`GET /api/session` tells you who you are, and `GET /api/logout` ends the session. After a logout the cookie is worthless:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/session
{"username":"admin","role":"admin","cert_expiry_warning":"14days"}
```

Sessions live in memory. A restart of r3v3rs3 ends every session, so a long-running script must handle a 401 by signing in again.

## Step 2: Read the API Document

r3v3rs3 generates the OpenAPI document from the server code, so it always describes the running version:

- OpenAPI document: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

Both need the session cookie. Open them in the browser where you signed in to the WebUI; the API link in the WebUI footer opens the Swagger UI.

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/openapi.json | jq '.info.version, (.paths | length)'
"1.0.1"
34
```

Use the document to generate a client, or to see the exact body of an endpoint instead of guessing it.

## Step 3: Create a Port

A port takes a name and a multiaddr:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/ports \
    -H 'Content-Type: application/json' \
    -d '{"name":"api-demo","listen":"/ip4/127.0.0.1/tcp/8475/http"}'
null
```

`null` is the success body. Read the id back from the list:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/ports | jq -r '.[] | select(.name=="api-demo") | .id'
smc-gzh
```

r3v3rs3 generates the id. Never write one by hand: a `PUT` with an id that does not exist is accepted and creates nothing that your proxies point at.

`PUT /api/ports/{id}` replaces a port, `DELETE /api/ports/{id}` removes it, and `GET /api/ports/{id}/status` reports whether the socket listens.

## Step 4: Create a Proxy

A proxy names the ports it runs on, its protocol and its routes:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/proxies \
    -H 'Content-Type: application/json' \
    -d '{
      "name": "api-demo-app",
      "ports": ["smc-gzh"],
      "protocol": "http",
      "vhosts": ["demo.example.com"],
      "routes": [{ "path": "/", "servers": [{ "url": "http://127.0.0.1:9000/" }] }]
    }'
null
```

The proxy answers at once, without a restart:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: demo.example.com' http://127.0.0.1:8475/
200
```

`protocol` selects the proxy type: `http`, `tcp` or `udp`. A TCP or UDP proxy carries `upstream_servers` instead of `routes`.

`GET /api/proxies/{id}/status` reports the upstream servers:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies/jzr-pgf/status
{"state":"active","upstreams":[{"addr":"http://127.0.0.1:9000/","weight":1,"healthy":true,"failures":0}]}
```

## Step 5: Purge the Cache After a Deploy

A deploy that changes static files leaves stale bodies in the HTTP cache. One call clears the cache of one proxy:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/jzr-pgf/cache
null
```

A whole deploy job needs three lines:

```bash
#!/bin/sh
set -eu
curl -sf -c "$PWD/j.txt" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$R3_USER\",\"method\":\"password\",\"password\":\"$R3_PASS\"}" \
  "$R3_URL/api/login" > /dev/null
curl -sf -b "$PWD/j.txt" -X DELETE "$R3_URL/api/proxies/$R3_PROXY/cache" > /dev/null
curl -sf -b "$PWD/j.txt" "$R3_URL/api/logout" > /dev/null
```

Keep the password in the secret store of your CI, never in the repository. `-f` makes `curl` fail the job on a 4xx or 5xx status.

## Step 6: Give the Job Its Own Account

Do not give a deploy job the admin password. Create an account with the role the job needs:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"ci","password":"a-long-password","role":"editor"}'
```

The three roles:

| Role | What it can do |
|---|---|
| `admin` | Everything, including accounts, settings and the audit log. |
| `editor` | Changes the ports and proxies of its own proxy list. |
| `viewer` | Reads only. |

A `viewer` reads the proxy list but cannot change anything:

```bash
$ curl -s -b ci.txt -X DELETE http://localhost:46492/api/proxies/jzr-pgf/cache
{"message":"the role of the account does not allow this action","error":{"message":"forbidden"}}
```

That request answers 403. A cache purge needs at least `editor`. An `editor` cannot read the audit log or the accounts; both answer 403. See [Accounts](@/accounts.md) for the per-account proxy list and TOTP.

## Step 7: Read the Audit Log

Every change through the WebUI or the API is recorded. Only an admin account can read it:

```bash
$ curl -s -b cookies.txt 'http://localhost:46492/api/audit?limit=5' | jq -c '.[]'
{"time":1789647517635,"username":"admin","client":"127.0.0.1","action":"purge_proxy_cache","resource_id":"jzr-pgf"}
{"time":1789647512463,"username":"admin","client":"127.0.0.1","action":"add_proxy","resource_id":"jzr-pgf","summary":"api-demo-app"}
{"time":1789647508125,"username":"admin","client":"127.0.0.1","action":"add_port","resource_id":"smc-gzh","summary":"api-demo /ip4/127.0.0.1/tcp/8475/http"}
{"time":1789647489459,"username":"admin","client":"127.0.0.1","action":"login"}
```

`time` is in milliseconds since the Unix epoch. The summary never holds a password, a token or a key. See [Audit Log](@/configuration.md#audit-log) for the query parameters.

## Step 8: Watch the Changes

`GET /api/events` is a server-sent event stream that the WebUI uses to refresh itself. Each change sends one line:

```bash
$ curl -N -b cookies.txt http://localhost:46492/api/events
data: {"event":"proxies_updated","entries":[...]}

data: {"event":"proxy_status_updated","id":"pdb-khh","status":{"state":"active"}}

data: {"event":"port_table_updated","entries":[...]}

data: {"event":"port_status_updated","id":"dpv-ylj","status":{"state":{"socket":"listening","tls":null}}}
```

Use it for a dashboard or an alert instead of asking the status endpoints in a loop.

## Reference

- [Admin API](@/configuration.md#admin-api): the address of the document and the sign-in.
- [Audit Log](@/configuration.md#audit-log): the fields and the retention.
- [Accounts](@/accounts.md): roles, proxy lists and TOTP.

## Next Steps

- [Load Balancing and Health Checks](@/tutorials/load-balancing.md): the status endpoint in practice.
- [Caching and Compression](@/tutorials/cache-and-compression.md): what the purge removes.
- [High Availability](@/tutorials/high-availability.md): the same API on every node of a cluster.
