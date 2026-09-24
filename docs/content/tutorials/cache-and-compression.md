+++
title = "Caching and Compression"
description = "Answer repeated requests from memory and send smaller responses"
weight = 8
+++

# Caching and Compression

This guide makes a site faster with two proxy settings. The cache answers a repeated request from memory, without a request to the upstream server. Compression sends a smaller body to each client.

The two work together: r3v3rs3 stores one unencoded response and compresses it for each client, so one stored entry serves a Brotli client, a gzip client and a client without compression.

Follow [Getting Started](@/tutorials/getting-started.md) first. This guide continues from a working proxy.

## Step 1: Turn On Compression

1. Open the proxy and find the **Compression** section.
2. Select **Brotli**, **Zstandard** and **Gzip**. The order of your selection is the order that r3v3rs3 prefers when a client accepts several encodings with the same `q` value.
3. Leave **Minimum Size (Bytes)** at `1024`. A smaller body gets no smaller through compression.
4. Click **Update**.

```bash
$ curl -s http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
12591

$ curl -s -H 'Accept-Encoding: br' http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
711
```

r3v3rs3 compresses a response only when the media type is in **Media Types**. The default list holds `text/*`, `application/json`, `application/javascript`, `application/manifest+json`, `application/wasm`, the XML types, `image/svg+xml` and two font types. An image or a video is already compressed, so it is not in the list.

r3v3rs3 skips compression when the upstream server already encoded the response, when `Cache-Control` holds `no-transform`, and for `text/event-stream`, because compression holds server-sent events back.

## Step 2: Turn On the Cache

1. Open the proxy and find the **Cache** section.
2. Turn on **Enable Cache**.
3. Leave **Memory Limit (Bytes)** at `67108864` (64 MiB) and **Maximum Response Size (Bytes)** at `1048576` (1 MiB).
4. Write `300` in **Default TTL (Seconds)**.
5. Click **Update**.

Every route of the proxy shares one cache. Each proxy has its own.

```bash
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS

$ curl -sI http://app.example.com/data.json | grep -iE 'x-cache|age'
x-cache: HIT
age: 0
```

`X-Cache: MISS` means the response came from the upstream server, `HIT` means it came from memory. `Age` counts the seconds since the upstream server sent the response.

## Step 3: Understand the TTL

The lifetime of a stored response comes from the first of these that exists: `s-maxage`, `max-age`, `Expires`, then **Default TTL (Seconds)**.

So the upstream server decides, and **Default TTL (Seconds)** covers the responses that say nothing. Set the headers in your application when you can:

```text
Cache-Control: public, max-age=300
```

With **Default TTL (Seconds)** at `0`, r3v3rs3 stores a response without those headers only when it has an `ETag` or a `Last-Modified` header, and revalidates it on every request. That costs one conditional request per client request, but it never serves stale content.

When a stored response is stale and has a validator, r3v3rs3 sends a conditional request with `If-None-Match` or `If-Modified-Since`. A `304 Not Modified` answer refreshes the stored headers, and the client gets the stored body without a new download.

## Step 4: Know What Is Not Stored

r3v3rs3 stores a response only when every condition holds. The usual reasons for a permanent `MISS`:

- The request is not `GET` or `HEAD`, or it carries `Range`, `Upgrade` or `Cache-Control: no-store`.
- The response holds a `Set-Cookie` header. A session response must not be shared.
- `Cache-Control` holds `no-store` or `private`.
- The response holds `Vary: *`.
- The request holds an `Authorization` header, and `Cache-Control` holds none of `public`, `s-maxage`, `must-revalidate`.
- The body is larger than **Maximum Response Size (Bytes)**.
- The status is not one of `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414`, `501`.

The cache key is the requested host with the path and the query. Load balancing does not change the key, so every upstream server shares one entry. When the response holds a `Vary` header with request header names, r3v3rs3 serves the stored response only to a request with the same values of those headers.

## Step 5: Purge After a Deploy

A deploy replaces the files, but the stored responses keep their TTL.

- Click **Purge** in the proxy list.
- Or send `DELETE /api/proxies/{id}/cache`.

```bash
$ curl -b session.txt -X DELETE http://127.0.0.1:46492/api/proxies/hsy-cns/cache
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS
```

A restart of r3v3rs3 and a change of the cache settings also remove every stored response. The stored responses live in memory only.

## Step 6: See the Real Client IP Behind a CDN

A CDN in front of r3v3rs3 caches too, and it hides the visitor address from the proxy. Without the right setting the rate limit and the IP filter see the edge server.

r3v3rs3 trusts the IP ranges of eight known CDNs by default: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva and Google Cloud Load Balancing. For a trusted peer it reads the provider header, for example `CF-Connecting-IP`, and sends the resolved address to the upstream server in `X-Real-IP`.

- The ranges are compiled into the binary and downloaded again every day. **Settings** shows the state of the list and has a **Refresh Now** button.
- Add your own load balancer to **Trusted Proxies** of the proxy.
- Akamai publishes no edge ranges. Add your Site Shield ranges to **Trusted Proxies**.

## In a Cluster

Each node has its own cache. With `share_cache = true` the nodes also store the responses in the cluster store, so a response that one node fetched serves the others.

- A node stores a response in the cluster store when it stays fresh for at least 60 seconds.
- A purge removes the shared responses too, and every node purges its local cache.
- Each stored response is a write to the store, so a busy cache grows the store between compactions.

[Shared Cache](@/cluster.md#shared-cache) describes the limits.

## Next Steps

- [Cache](@/configuration.md#cache) and [Compression](@/configuration.md#compression): every field and every condition.
- [Client IP](@/configuration.md#client-ip): the header order behind a CDN.
- [Load Balancing and Health Checks](@/configuration.md#load-balancing-and-health-checks): more upstream servers behind one cache.
