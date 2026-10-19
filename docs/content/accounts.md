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
| Read the apps, the targets, the deployments and the container logs of the deployment platform | yes | yes | no | yes, without a proxy list |
| Add, change or delete an app, read or change its environment variables, set or delete its Git token, create or delete its webhook secret, deploy an app, roll back a deployment | yes | yes | no | no |
| Add an agent target, create its enrollment token, delete it | yes | no | no | no |
| Read the Git provider connections, list the repositories and the branches of a connection, install the webhook of an app again | yes | yes | no | no |
| Add, change, connect or delete a Git provider connection, create or install a GitHub App | yes | no | no | no |

- An account without a proxy list sees every proxy. An admin always sees every proxy, so an admin cannot have a proxy list.
- A proxy that is not in the list of an account does not exist for the account. The admin API answers `404 id_not_found`.
- An action that the role does not allow gets `403 forbidden`.
- The WebUI hides the pages that the role cannot open.
- An editor with a proxy list can select an access list for its proxies. Only an account that changes the access lists can change the content of an access list.
- When an account deletes a proxy, r3v3rs3 removes the proxy from the proxy list of each account.
- An account with a proxy list gets `403 forbidden` for every request to the deployment platform.
- The admin API never returns the value of a secret environment variable.
- An account that can change an app can take over the host through the Compose file of a [Compose app](@/platform.md#compose-source), because r3v3rs3 does not restrict the settings of a Compose file. On an [agent target](@/platform.md#agent-targets) this is the host of the agent.

## Rules

- A password needs at least 8 characters.
- A username has 1 to 64 characters. It cannot contain `:`, `/`, a space or a control character.
- At least one admin account remains. A change that removes the last admin gets `400 last_admin`.
- An account cannot delete itself or change its own role. The admin API answers `400 cannot_change_own_account`.
- A change of the role, the proxy list or the password ends the sessions that started before the change. This applies to the admin panel sessions and to the Admin Session sign-ins of the proxies.
- A deleted account loses its sessions.

`accounts.toml` in the configuration directory holds the accounts with their password hashes. r3v3rs3 writes the file with mode `0600`.

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

## WebAuthn and Passkeys

r3v3rs3 supports hardware security keys, Touch ID, Face ID, and biometric device passkeys (FIDO2 / WebAuthn, ES256 P-256) for admin authentication alongside passwords and TOTP.

### Adding a Passkey

1. Log in to the WebUI and navigate to the **Accounts** page.
2. Edit your account or click **Add Passkey** on the passkey management section.
3. Enter a descriptive label (e.g., "MacBook Touch ID" or "YubiKey 5C") and confirm the browser's biometric/security key prompt.
4. r3v3rs3 registers the public key and assigns a monotonic counter for replay protection.

> [!NOTE]
> WebAuthn requires a secure origin (HTTPS) or `http://localhost`. When accessing the panel through a reverse proxy domain or loopback port, the Relying Party ID (`rp.id`) is automatically matched to the browser's current domain name or `localhost`. Web browsers do not permit passkeys on raw IP addresses (such as `127.0.0.1`).

### Signing in with a Passkey

On the login screen, click **Sign in with Passkey**. Supported browsers will prompt for resident key authentication without needing to type the username or password. You can also sign in directly by specifying your username and confirming your passkey.

### API Endpoints

| Endpoint | Method | Action |
|---|---|---|
| `/api/auth/passkey/challenge` | `GET` | Obtains a single-use random challenge for WebAuthn authentication or registration. |
| `/api/auth/passkey/register` | `POST` | Registers a newly created passkey credential for the signed-in caller. |
| `/api/auth/passkey/{id}` | `DELETE` | Deletes a registered passkey by its credential ID. |
| `/api/auth/login` | `POST` | Authenticates using `LoginMethod::Passkey` with signed WebAuthn assertion and client data. |

The [audit log](@/configuration.md#audit-log) records each change of an account and each sign-in.
