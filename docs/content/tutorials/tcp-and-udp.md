+++
title = "TCP and UDP Proxies"
description = "Publish a database over TCP and a DNS server over UDP, with mutual TLS"
weight = 9
+++

# TCP and UDP Proxies

r3v3rs3 proxies more than HTTP. This guide publishes a PostgreSQL server over TCP and a DNS server over UDP, and then puts mutual TLS in front of the database so that only a client with a certificate connects.

A TCP or UDP proxy forwards bytes. It reads no request, so it has no virtual hosts, no routes, no path and no header rules.

Follow [Getting Started](@/tutorials/getting-started.md) first.

## Step 1: Publish PostgreSQL over TCP

1. Click **Ports** in the menu, then **Add**.
2. Write `postgres` in the name field, select the interface and the port `5432`, and select the protocol **TCP**.
3. Click **Create** and check the state **Listening**.
4. Click **Proxies**, then **Add**, and select the protocol **TCP / TCP over TLS**.
5. Select the `postgres` port.
6. Under **Upstream Server**, write the address of the database in **Host**, for example `10.0.0.5`, and `5432` in **Port**.
7. Click **Create**.

```bash
$ psql "postgresql://postgres:<password>@proxy.example.com:5432/postgres" -c 'select version();'
 PostgreSQL 17.11 on aarch64-unknown-linux-musl, ...
```

r3v3rs3 stores the address as a multiaddr: `/ip4/<address>/tcp/<port>`, `/ip6/<address>/tcp/<port>` or `/dns/<name>/tcp/<port>`. With `/dns`, r3v3rs3 resolves the name for each connection.

A TCP port serves one TCP proxy at a time. A second TCP proxy on the same port is an error.

## Step 2: Publish a DNS Server over UDP

1. Add a port with the protocol **UDP**, for example on the port `53`.
2. Add a proxy with the protocol **UDP** and select the port. Under **Upstream Server**, write `10.0.0.53` in **Host** and `53` in **Port**.

```bash
$ dig +short @proxy.example.com example.com A
172.66.147.243
```

A UDP proxy opens one session for each client address, with its own socket to the upstream server. The session closes when no packet passes in either direction for **Session Idle Timeout (Seconds)**, 60 seconds by default. A port keeps at most 10,000 sessions and drops the packets of new clients when it is full.

## Step 3: Add Mutual TLS

A TCP over TLS port terminates TLS and sends plain bytes to the upstream server. With client authentication, only a client with a certificate of your CA connects.

### Create the certificates

1. Click **Certificates**, then the **Server Certs** tab, then **Self-sign**.
2. Write the host name of the proxy in the subject names, for example `db.example.com`, and create it. r3v3rs3 also creates a CA certificate in the **Root Certs** tab.
3. Open the **Client Certs** tab, click **Self-sign**, select the certificate type **Client Certificate** and the CA of the step before, then create it.
4. Download the client certificate with **Download**. The archive holds `chain.pem` and `key.pem`. Give both to the client.
5. Download the CA certificate in the **Root Certs** tab and save it as `ca.pem`. The client checks the server certificate with it.

### Bind the port

1. Add a port with the protocol **TCP over TLS**, for example on the port `5433`. The port `5432` of Step 1 already carries a TCP proxy.
2. Write the host name in **Server Names**. r3v3rs3 selects the server certificate from the SNI of the client.
3. Select **Required** in **Client Authentication**.
4. Select the CA certificate in **Client CA Certificates**. The system root certificates are not used.
5. Create the proxy on this port, with the same upstream server as Step 1.

Test the port with `openssl s_client`:

```bash
$ openssl s_client -connect db.example.com:5433 -servername db.example.com \
    -CAfile ca.pem -cert chain.pem -key key.pem
```

A client without `-cert` and `-key` gets no connection. The TLS library of the client names the alert.

**Optional** accepts a client without a certificate, and checks the certificate of a client that sends one. **Off** asks for none.

When the client authentication config becomes invalid, for example after somebody removes the root certificate, the port closes every connection and the port list shows **TLS Error**.

## Step 4: Encrypt the Upstream Connection

Steps 1 and 3 send plain bytes to the database. To use TLS there too:

1. Open the proxy and turn on **Connect with TLS** for the upstream server. Its address then ends with `/tls`.
2. When the database asks for a client certificate, select one in **Client Certificate**. r3v3rs3 sends it to every TLS upstream server that asks for one.

```toml
[my-database]
protocol = "tcp"
client_cert = "a1b2c3d"
upstream_servers = [{ addr = "/dns/db.internal/tcp/5433/tls" }]
```

A client certificate that a proxy uses cannot be deleted. When it becomes invalid, a TCP proxy closes the connection instead of connecting without it.

## Step 5: Add More Upstream Servers

A TCP proxy with several upstream servers balances the connections. A UDP proxy sends the packets of one session to one server.

- **Load Balancing** selects the algorithm, for example round robin or client IP hash.
- The health check removes a server that does not answer, and brings it back when it answers again. **Max Fails** and **Check Interval (Seconds)** set it.
- **Circuit Breaker** stops new connections to a server that keeps failing. When every server has an open circuit, the client connection closes.

[Load Balancing and Health Checks](@/configuration.md#load-balancing-and-health-checks) describes the fields.

## The Client Address

A TCP proxy hides the client address from the upstream server, because the connection comes from r3v3rs3.

- Turn on **Send PROXY Protocol** so that each upstream connection starts with the client address. Turn it on only when every upstream server reads that header, for example PostgreSQL behind a pooler that supports it.
- In the other direction, **Receive PROXY Protocol** on the port reads the address from a load balancer in front of r3v3rs3. A UDP port cannot do this.

## What a TCP or UDP Proxy Does Not Have

- No virtual hosts and no routes. One port carries one TCP proxy, so the port number selects the service.
- No cache, no compression and no header rules. These need HTTP.
- No authentication and no access list. Use mutual TLS, an IP filter of your firewall, or the authentication of the service itself.

## Next Steps

- [Ports](@/configuration.md#ports): every port type, the server names and the PROXY protocol.
- [UDP Sessions](@/configuration.md#udp-sessions): the session model and its limits.
- [Upstream Client Certificates](@/configuration.md#upstream-client-certificates): mutual TLS towards the upstream server.
