+++
title = "HTTPS with a Wildcard Certificate"
description = "Serve a domain and its subdomains over HTTPS with DNS-01"
weight = 6
+++

# HTTPS with a Wildcard Certificate

This guide serves `example.com` and every subdomain over HTTPS. r3v3rs3 orders one wildcard certificate from Let's Encrypt with the DNS-01 challenge, redirects HTTP to HTTPS, and sends HSTS.

DNS-01 is the only challenge that gets a wildcard certificate. It also needs no open port for the validation, so it works on a server behind a firewall.

## Before You Start

- A domain whose DNS zone you host at one of the providers of the [DNS-01 table](@/configuration.md#dns-01), for example Cloudflare or Route 53.
- An API credential of that provider that can read the zone and write records.
- A running r3v3rs3 with an admin account. [Getting Started](@/tutorials/getting-started.md) covers the install.
- The `A` record of `example.com` and of each subdomain points to the IP address of the server. DNS-01 does not check this, but your clients need it.

## Step 1: Create the API Credential

Create a credential that can write the `_acme-challenge` TXT records of the zone. r3v3rs3 creates each record before the validation and deletes it after.

| Provider | Credential | Permissions |
|---|---|---|
| Cloudflare | API Token | `Zone:Read` and `DNS:Edit` for the zone |
| Route 53 | Access Key ID, Secret Access Key | `route53:ListHostedZones` and `route53:ChangeResourceRecordSets` |

The [DNS-01 table](@/configuration.md#dns-01) lists the other 13 providers with their credentials. Two of them need no hosting provider: the webhook provider calls a service of your own, and the exec provider runs a program on the r3v3rs3 host.

## Step 2: Bind the HTTP and HTTPS Ports

The wildcard certificate needs no open port, but your site does.

1. Click **Ports** in the menu, then **Add**.
2. Select the interface `0.0.0.0`, the port `443` and the protocol **HTTPS**. Click **Create**.
3. Add a second port with the port `80` and the protocol **HTTP**. Step 5 redirects it.
4. Check that both ports show the state **Listening**.

On Linux a port below 1024 needs root or the `CAP_NET_BIND_SERVICE` capability.

## Step 3: Create the ACME Entry

1. Click **Certificates** in the menu, then the **ACME** tab, then **Add**.
2. Select the provider **Let's Encrypt**. The page uses its directory URL. **ACME Provider** offers Google Trust Services, ZeroSSL and a custom server too.
3. Write your address in **Email Address**. The certificate authority sends the expiry warnings there.
4. Write `example.com, *.example.com` in **Domain Names**. The certificate holds every name as a Subject Alternative Name.
5. Select **DNS-01** in **Challenge**.
6. Select your provider in **DNS Provider** and fill in the credential fields of Step 1.
7. Click **Create**. The preset providers order again 60 days after each order. A custom server has a **Renewal Interval (days)** field.

r3v3rs3 orders the certificate at once, and then at each renewal check. `example.com` and `*.example.com` share one TXT record name, `_acme-challenge.example.com`, so the record set holds two values during the validation.

r3v3rs3 stores the entry in `acme.toml`:

```toml
[abc-def]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"

[abc-def.dns_provider]
provider = "cloudflare"
api_token = "<token>"
```

Create the entry in the WebUI and not in this file, because r3v3rs3 creates the ACME account when you add the entry. The file holds the account key and the credential in plain text with the mode `0600`. The admin API never returns them.

## Step 4: Check the Certificate

The **Server Certs** tab shows the certificate after the order. It names the issuer, the subject names and the **Renews on** date.

A failed order writes the reason to the server log, and r3v3rs3 orders again one hour later. Two causes are common:

- The credential cannot write the zone. The log names the API error of the provider.
- The TXT record is not visible after 5 minutes. A cached answer of the system resolver causes this. Set **DNS Challenge Resolver** in **Settings** to `1.1.1.1:53` or to an authoritative name server of the zone.

## Step 5: Create the Proxy

1. Click **Proxies** in the menu, then **Add**.
2. Select the protocol **HTTP / HTTPS**.
3. Select **both** ports of Step 2. The proxy needs the HTTP port for the redirect and the HTTPS port for the traffic.
4. Write `app.example.com` in **Virtual Hosts**. The wildcard certificate covers every subdomain, so each subdomain can get its own proxy.
5. Write the address of your application in **Target**, for example `http://127.0.0.1:3000`.
6. Turn on **Automatically Redirect HTTP to HTTPS**.
7. Click **Create**.

r3v3rs3 selects the certificate from the SNI name of the TLS handshake, so the HTTPS port needs no certificate setting.

Check both protocols:

```bash
$ curl -i http://app.example.com/
HTTP/1.1 301 Moved Permanently
location: https://app.example.com/

$ curl -i https://app.example.com/
HTTP/2 200
```

The redirect keeps the path and the query. It runs before the redirect rules and before authentication.

## Step 6: Send HSTS

HSTS tells a browser to use HTTPS for the next request without a redirect. Add it as a response header rule:

1. Open the proxy and find the **Header Rules** section.
2. Write this line in **Response Headers**:

```text
set Strict-Transport-Security: max-age=31536000; includeSubDomains
```

3. Save.

```bash
$ curl -sI https://app.example.com/ | grep -i strict
strict-transport-security: max-age=31536000; includeSubDomains
```

- The rule changes the responses of the upstream server. It does not change the responses that r3v3rs3 creates itself, such as the HTTP redirect and the error pages. A browser needs one successful HTTPS response to store the policy, so this is enough.
- `max-age=31536000` is one year. A browser refuses plain HTTP for the domain during that time. Start with a short `max-age`, for example `300`, until you are sure that every subdomain serves HTTPS.
- `includeSubDomains` covers every subdomain. Do not send it while a subdomain still serves plain HTTP.

## Renewal

r3v3rs3 orders a new certificate `renewal_days` after the last order, 60 days with a preset provider. A Let's Encrypt certificate is valid for 90 days, so the renewal has 30 days of margin. The order repeats the DNS-01 challenge, so the credential must stay valid. In a cluster only the leader orders certificates.

The **Settings** page has a notification webhook. It sends a `certificate_expiring` event before an expiry and an `acme_order_failed` event after a failed order, so a broken credential does not stay silent. [Notifications](@/configuration.md#notifications) describes the events.

## Next Steps

- [Certificates and ACME](@/configuration.md#acme): every challenge, every provider and the stored data.
- [Header Rules](@/configuration.md#header-rules): the other rules and their variables.
- [Proxies from Docker](@/tutorials/docker-discovery.md): a container label can name this ACME entry, so a discovered proxy gets its own certificate.
