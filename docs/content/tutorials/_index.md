+++
title = "Tutorials"
description = "Step by step guides that build a working setup"
sort_by = "weight"
weight = 0
+++

# Tutorials

Each page here is a walkthrough. Follow it from the top and you get a working setup. The other pages of this site are references: they describe every field and every behavior, but they do not build anything.

- [Installing on a Linux Server](@/tutorials/install-linux.md): `install.sh` and the systemd service.
- [Installing with Docker](@/tutorials/install-docker.md): one container with two volumes.
- [Getting Started](@/tutorials/getting-started.md): the first admin account, the first port and the first proxy.
- [High Availability](@/tutorials/high-availability.md): three nodes with a shared state in etcd or Consul, behind a load balancer.
- [Proxies from Docker](@/tutorials/docker-discovery.md): proxies built from the labels of your containers.
- [HTTPS with a Wildcard Certificate](@/tutorials/https-certificates.md): a DNS-01 wildcard certificate, the HTTPS redirect and HSTS.
- [Protecting an Application](@/tutorials/protect-an-app.md): access list, authentication, rate limit and audit log.
- [Caching and Compression](@/tutorials/cache-and-compression.md): responses from memory and smaller bodies.
- [TCP and UDP Proxies](@/tutorials/tcp-and-udp.md): a database, a DNS server and mutual TLS.
- [r3v3rs3 on Kubernetes](@/tutorials/kubernetes.md): the ingress controller, Ingress resources and the R3v3rs3Proxy custom resource.
- [Load Balancing and Health Checks](@/tutorials/load-balancing.md): weights, health checks, retries, sticky sessions, WebSocket and gRPC.
- [Driving r3v3rs3 from a Script](@/tutorials/admin-api.md): the admin API, a deploy job and an account for it.
- [Proxies from Consul and etcd](@/tutorials/consul-etcd-discovery.md): the catalog, the key-value store and etcd keys.
- [Accounts for a Team](@/tutorials/team-accounts.md): roles, proxy lists, TOTP and the audit log.
- [Single Sign-On with Forward Auth](@/tutorials/forward-auth.md): oauth2-proxy in front, the user in the upstream request.
- [Migrating from nginx or Traefik](@/tutorials/migrate.md): the equivalents of each directive and the cutover order.
