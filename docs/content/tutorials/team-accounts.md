+++
title = "Accounts for a Team"
description = "Give each person the proxies they own, turn on TOTP and read who changed what"
weight = 14
+++

# Accounts for a Team

One admin password shared by five people is not an access model. This guide gives each person an account with the role they need, limits an account to its own proxies, turns on TOTP, uses the same accounts to protect an application, and shows who changed what.

You need an admin account from [Getting Started](@/tutorials/getting-started.md).

## Step 1: Pick the Role

Three roles exist. A proxy list narrows an editor or a viewer further:

| Action | Admin | Editor | Editor with a proxy list | Viewer |
|---|---|---|---|---|
| Read the proxies | every proxy | every proxy | the proxies of its list | every proxy, or its list |
| Add a proxy | yes | yes | yes, and its list gets the new proxy | no |
| Change or delete a proxy, purge its cache | yes | yes | the proxies of its list | no |
| Read the ports, certificates, ACME entries, access lists | yes | yes | yes | yes |
| Change the ports, certificates, ACME entries, access lists | yes | yes | no | no |
| Read or change the settings and the accounts, read the audit log | yes | no | no | no |

An admin always sees every proxy, so an admin cannot have a proxy list. An account without a list sees every proxy.

## Step 2: Create an Account

Open **Accounts** in the WebUI. Only an admin sees the page. On the command line:

```bash
$ r3v3rs3 add-user alice --role editor
$ r3v3rs3 add-user bob --role viewer --totp
```

Without `--role` the account is an admin. The command asks for the password when `--password` is not set. An account from the command line has no proxy list; set one on the **Accounts** page or through the API.

Through the API, one call creates the account with its list:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"alice","password":"alice-long-password","role":"editor","proxies":["pdb-khh"],"totp":false}'
{}
```

A password needs at least 8 characters. A username has 1 to 64 characters and cannot contain `:`, `/`, a space or a control character.

## Step 3: Watch the Proxy List Work

Alice signs in and sees only her proxy:

```bash
$ curl -s -b alice.txt http://localhost:46492/api/proxies | jq -r '.[].id'
pdb-khh
$ curl -s -b alice.txt http://localhost:46492/api/session
{"username":"alice","role":"editor","proxies":["pdb-khh"],"cert_expiry_warning":"14days"}
```

A proxy outside her list does not exist for her:

```bash
$ curl -s -b alice.txt http://localhost:46492/api/proxies/cfr-kpm
{"message":"port id not found: cfr-kpm","error":{"message":"id_not_found","id":"cfr-kpm"}}
```

That is a 404, not a 403. An editor with a proxy list also cannot change the ports:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -b alice.txt -X POST http://localhost:46492/api/ports \
    -H 'Content-Type: application/json' -d '{"name":"x","listen":"/ip4/127.0.0.1/tcp/8479/http"}'
403
```

A proxy that she creates joins her list on its own:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/accounts | jq -c '.[] | select(.username=="alice")'
{"username":"alice","role":"editor","proxies":["pdb-khh","tpq-gcv"],"totp":false}
```

So an editor with a list can build and run their own service without ever seeing the proxies of another team.

## Step 4: Turn On TOTP

Create the account with `"totp": true`. TOTP can be set only when the account is created: to add it to an existing account, delete the account and create it again. The response holds the secret once:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","password":"bob-long-password","role":"viewer","totp":true}'
{"totp_secret":"V2J27IEPLYHJ3Y62XSZM4GJBQSFTFSRF"}
```

The **Accounts** page shows the secret once as well. Add it to the authenticator app at that moment; r3v3rs3 does not show it again.

The sign-in then takes two calls. The first answers `totp_required`:

```bash
$ curl -s -c bob.txt -X POST http://localhost:46492/api/login \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","method":"password","password":"bob-long-password"}'
"totp_required"
$ curl -s -b bob.txt -c bob.txt -X POST http://localhost:46492/api/login \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","method":"totp","token":"418205"}'
"success"
```

A wrong code answers 400 `invalid_login_credentials`, the same as a wrong password. `max_login_attempts` (default 10) blocks a client IP address and username pair after that many failures, and `login_attempts_reset` (default 15 minutes) unblocks it.

## Step 5: Protect an Application with the Same Accounts

An application without a sign-in of its own can use these accounts. Set the authentication of the proxy to **Admin Session**:

```bash
$ curl -s -b cookies.txt -X PUT http://localhost:46492/api/proxies/tpq-gcv \
    -H 'Content-Type: application/json' -d '{ ... , "auth": {"type":"session"} }'
```

A browser request without a session is redirected:

```
HTTP/1.1 302 Found
location: /.r3v3rs3/auth/login?redirect=%2F
```

Other methods receive 401. The sign-in form takes `username`, `password`, `totp` and `redirect`:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -c app.txt -X POST \
    http://app.example.com/.r3v3rs3/auth/login \
    -d 'username=alice&password=alice-long-password&redirect=/'
303
```

After that the application answers as usual. `POST /.r3v3rs3/auth/logout` ends the session, and the next request is redirected again.

**Only an account that sees the proxy can sign in.** An account with a proxy list that does not hold this proxy gets 401 on the sign-in form, with the right password. This is the part that surprises people: the proxy list controls the application access too, not only the panel.

r3v3rs3 checks the account on each request, so a session ends when the account is removed, when the account changes, or when the proxy leaves its list. Sessions live in memory, so a restart signs every client out.

## Step 6: Change and Remove an Account

A change of the role, the proxy list or the password ends the sessions that started before the change:

```bash
$ curl -s -b cookies.txt -X PUT http://localhost:46492/api/accounts/alice \
    -H 'Content-Type: application/json' -d '{"role":"viewer","proxies":["pdb-khh"]}'
null
$ curl -s -o /dev/null -w '%{http_code}\n' -b alice.txt http://localhost:46492/api/proxies
401
```

This applies to the panel sessions and to the application sign-ins of Step 5, so a person who leaves the team loses both at once.

Two rules stop you from locking yourself out:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/accounts/admin
{"message":"an account cannot delete itself or change its own role","error":{"message":"cannot_change_own_account"}}
```

- An account cannot delete itself or change its own role.
- At least one admin account remains. A change that removes the last admin answers 400 `last_admin`.

An update without `proxies` removes the proxy list, and an update without `password` keeps the password.

## Step 7: Read Who Changed What

Every change and every sign-in is recorded. Only an admin can read it:

```bash
$ curl -s -b cookies.txt 'http://localhost:46492/api/audit?limit=3' | jq -c '.[]'
{"time":1789647512463,"username":"admin","client":"127.0.0.1","action":"add_proxy","resource_id":"jzr-pgf","summary":"api-demo-app"}
{"time":1789647489459,"username":"admin","client":"127.0.0.1","action":"login"}
```

The **Audit Log** page of the WebUI filters by account, resource and period, and shows at most 500 entries. The summary holds names, addresses and roles, and never a password, a token or a key. The default retention is one year.

The changes that r3v3rs3 makes by itself, for example a certificate renewal or a discovered proxy, are not recorded.

## Where the Accounts Live

`accounts.toml` in the configuration directory holds the accounts with their password hashes, written with mode `0600`. A cluster keeps the accounts encrypted in the store, so every node shares them.

## Reference

- [Accounts](@/accounts.md): every rule, every error code and the API.
- [Admin Session](@/configuration.md#admin-session): the endpoints and the cookie.
- [Audit Log](@/configuration.md#audit-log): the fields and the query parameters.

## Next Steps

- [Protecting an Application](@/tutorials/protect-an-app.md): IP filter, basic auth and rate limit.
- [Driving r3v3rs3 from a Script](@/tutorials/admin-api.md): an account for a deploy job.
