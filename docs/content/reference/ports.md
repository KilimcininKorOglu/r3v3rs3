+++
title = "Ports"
description = "Listening ports, TLS, PROXY protocol, port ranges and transparent mode"
weight = 2
+++

A proxy needs a port. Add one in the "Ports" section.

r3v3rs3 supports six types of ports:

- HTTP
- HTTPS (HTTP over TLS)
- HTTP over QUIC (HTTP/3)
- TCP
- TCP over TLS
- UDP

The `listen` address of a port starts with `/ip4/<address>` or `/ip6/<address>`, followed by `/tcp/<port>` (TCP), `/tcp/<port>/tls` (TCP over TLS), `/tcp/<port>/http` (HTTP), `/tcp/<port>/https` (HTTPS), `/udp/<port>` (UDP) or `/udp/<port>/quic/http` (HTTP over QUIC). A TCP or UDP port can name a range instead of one port, such as `/udp/49160-49240` (see "Port Ranges"). `name` is an optional name of the port. Turn off "Active" (`active = false`) to stop a port or a proxy without deleting it.

## Server Names

HTTPS, HTTP over QUIC and TCP over TLS ports have a "Server Names" field. r3v3rs3 selects the server certificate from the SNI (Server Name Indication) of the client. When the client sends no SNI, r3v3rs3 selects a valid server certificate that has all the server names. When the list is empty, r3v3rs3 selects the first valid server certificate.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS Client Authentication

HTTPS, HTTP over QUIC and TCP over TLS ports can verify the certificate of the client (mutual TLS). Select the mode in "Client Authentication":

| Mode     | Behavior                                                                                                                 |
|----------|--------------------------------------------------------------------------------------------------------------------------|
| Off      | The port does not ask for a client certificate. This is the default.                                                     |
| Optional | The port asks for a certificate but also accepts clients without one. A certificate that the client sends must be valid. |
| Required | The TLS handshake fails when the client sends no valid certificate.                                                      |

In Optional and Required mode, select one or more root certificates in "Client CA Certificates". A client certificate must be signed by one of them. The system root certificates are not used. r3v3rs3 rejects a port config without a root certificate, or with a certificate that is not a root certificate. A root certificate that a port uses cannot be deleted.

When the client authentication config becomes invalid, for example after the root certificate is removed from the configuration directory, the port closes every connection and the port list shows "TLS Error".

On HTTPS and HTTP over QUIC ports, header rules can send the verified certificate to the upstream server with the `{client_cert_subject}` and `{client_cert_fingerprint}` variables. See "Header Rules".

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

### Route-Level Client Certificate Requirement

When an HTTPS or HTTP over QUIC port uses `Optional` client authentication, individual routes can enforce client certificate verification with `require_client_cert = true`. Requests matching a route with `require_client_cert = true` without a verified client certificate are immediately rejected with `403 Forbidden`. This enables hosting public routes (such as `/`, `/docs`) and mTLS-restricted routes (such as `/admin`, `/internal`) on the very same port:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }] },
  { path = "/admin", servers = [{ url = "http://127.0.0.1:8080/admin" }], require_client_cert = true },
]
```

### TLS 1.3 0-RTT Early Data & Anti-Replay Mitigation

For mobile and high-latency clients, r3v3rs3 supports TLS 1.3 0-RTT Early Data on TLS-terminated ports (`early_data = true`). When enabled, returning clients with a valid session resumption ticket can send application data in their initial ClientHello flight, reducing round-trip connection latency to zero:

- **RFC 8470 Safe Methods Only**: Per RFC 8470 Section 5.2, safe HTTP methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`) are allowed in early data. State-changing requests (`POST`, `PUT`, `DELETE`, `PATCH`) sent in early data are rejected with HTTP `425 Too Early` and include a `Retry-After: 0` header, instructing clients to retry once the TLS handshake finishes.
- **In-Memory Anti-Replay Buffer**: Sliding-window anti-replay cache records 0-RTT request identifiers within a 60-second window. Any detected replay attempt is rejected with `425 Too Early`.
- **Upstream Forwarding**: Upstream requests arriving via 0-RTT carry an injected `Early-Data: 1` header. An `Early-Data` header that the client sends on a standard connection reaches the upstream unchanged, because RFC 8470 Section 5.1 forbids an intermediary to remove it.
- **One Request per 0-RTT Connection**: A connection that carried early data closes after its first response. The next request opens a new connection, so only the requests in the early data count as early.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], early_data = true }
```

Via the API (`PUT /api/ports/{id}/early_data` with `{"enabled": true}`), the CLI or MCP (`update_port_early_data`). The server refuses to turn early data on for a port without TLS termination:
```bash
r3 port update-early-data <PORT_ID> --enable true
r3 port update-early-data <PORT_ID> --enable false
```

## PROXY Protocol

A load balancer in front of r3v3rs3 can send the client address in a PROXY protocol header (version 1 or 2). TCP, TCP over TLS, HTTP and HTTPS ports can read this header. UDP and HTTP over QUIC ports cannot.

Enable "Receive PROXY Protocol" and list the IP addresses or CIDR blocks of the load balancers in "Trusted Load Balancers":

- A connection from a trusted address must start with a valid header. r3v3rs3 closes the connection when the header is invalid, when "Accepted Versions" does not include its version, or when the header does not arrive in "Header Timeout". The default timeout is 5 seconds.
- r3v3rs3 does not read a header from other addresses. The client address of such a connection is the peer address.
- A version 2 `LOCAL` header and a version 1 `UNKNOWN` header keep the peer address. Load balancers send them in health checks.

r3v3rs3 reads the header before the TLS handshake. The address from the header is the client address of the connection. IP filters, rate limits, the client IP hash, the `Forwarded` and `X-Forwarded-For` headers and the access log use it. The access log also writes the peer address in the `peer` field.

An ACME HTTP-01 challenge request that starts with a PROXY protocol header does not receive the challenge response. Use the DNS-01 challenge for a certificate that such a load balancer serves.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"] }
proxy_protocol = { trusted = ["10.0.0.0/8"], accept = "v2", timeout = "5s" }
```

`accept` is `any` (the default), `v1` or `v2`.

## Port Ranges

A TCP or UDP port can listen on a range of ports, for example the relay ports of a TURN server: `/ip4/0.0.0.0/udp/49160-49240`. A range holds at most 16384 ports. TCP over TLS, HTTP, HTTPS and HTTP over QUIC ports take a single port only.

- Each port of the range goes to the upstream port at the same position. With the range `49160-49240` and the upstream `/ip4/10.0.0.5/udp/50000-50080`, a packet to port 49165 goes to port 50005. Every upstream server of the proxy needs a range of the same size; r3v3rs3 refuses a single upstream port or a range of another size.
- A range cannot cover a port that another TCP or UDP port of the same protocol or the SSH server listens on.
- r3v3rs3 opens one socket on the first port and keeps an nftables table (`inet r3v3rs3_tproxy`) and a policy route (firewall mark `0x7233`, routing table 7233) that send the whole range to it with TPROXY. A UDP reply leaves from the port the client sent to.
- The table and the route need `CAP_NET_ADMIN`. `install.sh` gives it to the `r3-server` unit. r3v3rs3 removes the table and the route when the last range or transparent port goes away.
- The TPROXY rule runs before the Docker port forwarding, so a range also takes the packets of a published container port inside it.
- When the host firewall table is on, it opens the whole range.

```toml
[turn-relay]
listen = "/ip4/0.0.0.0/udp/49160-49240"
```

## Transparent Mode

By default an upstream server sees the address of r3v3rs3 as the source of every connection. Turn on "Transparent Mode" (`transparent = true`) on a TCP, TCP over TLS or UDP port when the upstream must see the address of the client, for example a TURN server that builds its relay addresses from it. The option also works on a single port. HTTP, HTTPS and HTTP over QUIC ports refuse it; they pass the client address in the `Forwarded` and `X-Forwarded-For` headers instead.

- r3v3rs3 opens each upstream connection from the client address and marks it (firewall mark `0x7234`). The `inet r3v3rs3_tproxy` table and the policy route bring the replies back to r3v3rs3.
- The upstream must answer through this server: it runs on this host (a container or a host network service) or uses this server as its route back to the clients.
- An upstream on a loopback address (`127.0.0.1`, `::1`) cannot work, because Linux refuses a packet with a foreign source address to a loopback address. r3v3rs3 refuses such an upstream on a transparent port. Bind the upstream to another address of the host.
- The client and the upstream server need the same address family (IPv4 or IPv6).

```toml
[turn-relay]
listen = "/ip4/0.0.0.0/udp/49160-49240"
transparent = true
```

Via the API (`PUT /api/ports/{id}/transparent` with `{"enabled": true}`), the CLI or MCP (`update_port_transparent`):
```bash
r3 port update-transparent <PORT_ID> --enable true
r3 port update-transparent <PORT_ID> --enable false
```

To remove the network rules by hand after r3v3rs3 is uninstalled:
```bash
nft delete table inet r3v3rs3_tproxy
ip rule del fwmark 0x7233 lookup 7233
ip route flush table 7233
ip -6 rule del fwmark 0x7233 lookup 7233
ip -6 route flush table 7233
```

## Resetting a Port

Changing the port configuration does not affect existing connections. Old connections will continue to use the old configuration. To forcibly close existing connections, you can reset the port.
