+++
title = "Accounts"
description = "Roles and proxy lists of the panel accounts"
weight = 0
+++

# Accounts

Each panel account has a role. An editor or a viewer can also have a proxy list. The admin panel, the admin API and the [Admin Session](@/configuration.md#admin-session) authentication of the proxies use the same accounts.

## Roles

| Action | Admin | Editor | Editor with a proxy list | Viewer |
|---|---|---|---|---|
| Read the proxies | every proxy | every proxy | the proxies of its list | every proxy, or the proxies of its list |
| Add a proxy | yes | yes | yes, and its list gets the new proxy | no |
| Change or delete a proxy, purge its cache | yes | yes | the proxies of its list | no |
| Read the ports, the certificates, the ACME entries and the access lists | yes | yes | yes | yes |
| Change the ports, the certificates, the ACME entries and the access lists, download a certificate, refresh the CDN IP ranges | yes | yes | no | no |
| Read or change the settings and the accounts, read the audit log | yes | no | no | no |
| Read the apps, the targets and the deployments of the deployment platform | yes | yes | no | yes, without a proxy list |
| Add, change or delete an app, read or change its environment variables, set or delete its Git token, deploy an app, roll back a deployment | yes | yes | no | no |

- An account without a proxy list sees every proxy. An admin always sees every proxy, so an admin cannot have a proxy list.
- A proxy that is not in the list of an account does not exist for the account. The admin API answers `404 id_not_found`.
- An action that the role does not allow gets `403 forbidden`.
- The WebUI hides the pages that the role cannot open.
- An editor with a proxy list can select an access list for its proxies. Only an account that changes the access lists can change the content of an access list.
- When an account deletes a proxy, r3v3rs3 removes the proxy from the proxy list of each account.
- An account with a proxy list gets `403 forbidden` for every request to the deployment platform.
- The admin API never returns the value of a secret environment variable.

## Rules

- A password needs at least 8 characters.
- A username has 1 to 64 characters. It cannot contain `:`, `/`, a space or a control character.
- At least one admin account remains. A change that removes the last admin gets `400 last_admin`.
- An account cannot delete itself or change its own role. The admin API answers `400 cannot_change_own_account`.
- A change of the role, the proxy list or the password ends the sessions that started before the change. This applies to the admin panel sessions and to the Admin Session sign-ins of the proxies.
- A deleted account loses its sessions.

`accounts.toml` in the configuration directory holds the accounts with their password hashes. r3v3rs3 writes the file with mode `0600`. A cluster keeps the accounts encrypted in the store.

## Create an Account

The "Accounts" page of the WebUI lists, adds, changes and deletes the accounts. Only an admin opens the page. When you add an account with TOTP, the page shows the TOTP secret once. Add it to your authenticator app at that time.

On the command line, `add-user` adds an account. Without `--role`, the account is an admin:

```bash
$ r3v3rs3 add-user alice --role editor
$ r3v3rs3 add-user bob --role viewer --totp
```

`--role` accepts `admin`, `editor` or `viewer`. The command asks for the password when `--password` is not set. An account from the command line has no proxy list. Set a proxy list on the "Accounts" page or through the admin API.

## Admin API

Only an admin account can call these endpoints.

| Endpoint | Action |
|---|---|
| `GET /api/accounts` | Lists the accounts with the role, the proxy list and whether TOTP is on. |
| `POST /api/accounts` | Adds an account. The response holds `totp_secret` when TOTP is on. |
| `PUT /api/accounts/{username}` | Changes the role, the proxy list and, optionally, the password. |
| `DELETE /api/accounts/{username}` | Deletes the account. |

```bash
$ curl -b cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"alice","password":"correct horse","role":"editor","proxies":["a1b2c3d"],"totp":false}' \
    http://localhost:46492/api/accounts
$ curl -b cookies.txt -X PUT -H 'Content-Type: application/json' \
    -d '{"role":"viewer"}' \
    http://localhost:46492/api/accounts/alice
```

An update without `proxies` removes the proxy list, and an update without `password` keeps the password.

| Response | Cause |
|---|---|
| `400 password_too_short` | The password has fewer than 8 characters. |
| `400 invalid_username` | The username breaks the username rules. |
| `400 invalid_account_scope` | An admin account has a proxy list. |
| `409 account_exists` | An account with the username exists. |
| `404 account_not_found` | No account has the username. |

`GET /api/session` returns the username, the role and the proxy list of the signed-in account.

The [audit log](@/configuration.md#audit-log) records each change of an account and each sign-in.
