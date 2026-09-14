+++
title = "Service Discovery"
description = "Service Discovery"
weight = 0
+++

# Service Discovery

A discovery provider reads proxy definitions from an external system and adds them to the proxy list. When a definition changes, r3v3rs3 updates the routes without a restart.

## Discovered Proxies

- A discovered proxy is read-only. The admin API returns `proxy_read_only` for an update or a delete. The WebUI shows the proxy with its source and without the edit actions. To change the proxy, change its definition in the source.
- r3v3rs3 does not save discovered proxies to `proxies.toml`. The provider sends them again after a restart.
- The id of a discovered proxy comes from the provider and the key of the definition. The proxy keeps its id after a restart, so the logs and the status keep the same id.
- A provider does not open ports. A definition names existing ports in `ports`, by port name or by port id. A port name must select exactly one port, and the port must accept the protocol of the proxy. When you rename or delete a port, r3v3rs3 resolves the port names again.
- A discovered TCP proxy cannot use a TCP port that another TCP proxy uses.
- When a provider loses its connection, the proxies of its last read stay active.

## Provider Status

The proxy list shows the state of each provider, the number of proxies that it added and the definitions that did not become a proxy. The admin API returns the same data at `GET /api/discovery`.

| State | Meaning |
|---|---|
| `connecting` | The provider has not read its resources yet. |
| `running` | The provider has read its resources and watches them for changes. |
| `error` | The provider cannot read its resources. The proxies of its last read stay active. |

An issue names the resource and the reason, for example `web-1: http.app: port not found: https`.

# Labels

Docker labels, Consul tags and key-value entries use the same keys. Each key has this form:

```text
r3v3rs3.<protocol>.<name>.<field>=<value>
```

- `<protocol>` is `http`, `tcp` or `udp`.
- `<name>` identifies the proxy in the resource. One resource can define more than one proxy.
- `<field>` is a field of the proxy model of the admin API. A dot separates nested fields.
- r3v3rs3 skips labels without the `r3v3rs3.` prefix. An unknown `r3v3rs3.` label or an unknown field is an issue, and the proxy is not added.

## Proxy Keys

| Key | Value |
|---|---|
| `r3v3rs3.enable` | `true` selects the resource when the provider does not read every resource. |
| `ports` | Required. Port names or port ids, separated by commas. |
| `name` | The name in the proxy list. The default is `<name>`. |
| `active` | `false` adds the proxy as inactive. The default is `true`. |
| `port` | The port of the upstream server on the address of the resource. |
| `scheme` | `http` or `https` for `port` of an HTTP proxy. The default is `http`. |
| `acme` | The id of an ACME entry that orders a certificate for the `vhosts` of an HTTP proxy. See [ACME Certificates](@/discovery.md#acme-certificates). |

## Values

- A boolean is `true` or `false`. A number is a decimal number.
- A duration uses units, for example `500ms`, `30s` or `5m`.
- A list is a comma-separated value: `vhosts=app.example.com,www.example.com`.
- A list of groups uses the keys `0`, `1`, `2` and so on: `routes.0.path=/`, `routes.1.path=/api`. The numbers set the order.
- An empty value is no value.
- A comma-separated value is not available inside `auth` and header rules, because their fields are text. Use numbered keys there: `auth.users.0.username=alice`.

## HTTP Proxies

`port` and `scheme` define one route to `/` with one server. They cannot be used together with `routes`.

```text
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.port=8080
```

A route can also use `port` and `scheme`. They cannot be used together with `servers` of the same route.

```text
r3v3rs3.http.app.ports=http,https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.routes.0.path=/
r3v3rs3.http.app.routes.0.port=8080
r3v3rs3.http.app.routes.1.path=/api
r3v3rs3.http.app.routes.1.port=9443
r3v3rs3.http.app.routes.1.scheme=https
```

A server can have an explicit URL and a weight:

```text
r3v3rs3.http.app.routes.0.servers.0.url=http://10.0.0.5:8080
r3v3rs3.http.app.routes.0.servers.1.url=http://10.0.0.6:8080
r3v3rs3.http.app.routes.0.servers.1.weight=3
r3v3rs3.http.app.load_balancing=round_robin
```

The other fields use the names of the admin API:

```text
r3v3rs3.http.app.upgrade_insecure=true
r3v3rs3.http.app.ip_filter.allow=10.0.0.0/8,192.168.0.0/16
r3v3rs3.http.app.rate_limit.requests=100
r3v3rs3.http.app.rate_limit.per=minute
r3v3rs3.http.app.rate_limit.burst=20
r3v3rs3.http.app.timeouts.connect=3s
r3v3rs3.http.app.timeouts.request=30s
r3v3rs3.http.app.health_check.interval=10s
r3v3rs3.http.app.health_check.path=/healthz
r3v3rs3.http.app.compression.algorithms=br,gzip
r3v3rs3.http.app.cache.enabled=true
r3v3rs3.http.app.cache.default_ttl=1m
r3v3rs3.http.app.headers.response.0.action=set
r3v3rs3.http.app.headers.response.0.name=X-Frame-Options
r3v3rs3.http.app.headers.response.0.value=DENY
r3v3rs3.http.app.auth.type=basic
r3v3rs3.http.app.auth.realm=Staff
r3v3rs3.http.app.auth.users.0.username=alice
r3v3rs3.http.app.auth.users.0.password=change-me
```

r3v3rs3 replaces plain text passwords and tokens with hashes before it uses the proxy. The source still holds the plain text value, so prefer `password_hash` and `token_hash` in labels.

## TCP and UDP Proxies

`port` defines one upstream server on the address of the resource. It cannot be used together with `upstream_servers`.

```text
r3v3rs3.tcp.db.ports=postgres
r3v3rs3.tcp.db.port=5432

r3v3rs3.udp.dns.ports=dns
r3v3rs3.udp.dns.upstream_servers.0.addr=/ip4/10.0.0.53/udp/53
r3v3rs3.udp.dns.session_idle_timeout=30s
```

## ACME Certificates

`acme` names an existing ACME entry of the certificate list. r3v3rs3 orders a certificate for the virtual hosts of the proxy with the account, the challenge and the DNS provider of the entry.

```text
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com,www.example.com
r3v3rs3.http.app.port=8080
r3v3rs3.http.app.acme=abc-def
```

- The certificate has the virtual hosts as its domain names. It is a separate order of the entry, so it renews on its own schedule after the `renewal_days` of the entry. The certificates of the entry keep their schedule.
- r3v3rs3 does not order a certificate while a valid server certificate with a private key has every virtual host, for example a wildcard certificate of the entry. After the first certificate of the proxy, the renewal follows the proxy certificate.
- The proxies with the same entry and the same virtual hosts share one certificate.
- A missing or inactive entry, a proxy without virtual hosts, a regular expression virtual host and a name that the challenge of the entry cannot validate are issues, for example a wildcard virtual host with `http-01`. The proxy is added without a certificate.
- An inactive proxy does not order a certificate. A TCP or UDP proxy with `acme` is an issue.
- After a failed order, r3v3rs3 waits one hour before the next order.
- When the label is removed, the certificate is not renewed. r3v3rs3 removes it from the certificate list after it expires.
- An Ingress uses the `r3v3rs3.io/acme` annotation, and an R3v3rs3Proxy uses `spec.acme`.

# Docker

The Docker provider reads the labels of the running containers through the Docker Engine API. It follows the container events, so a started, stopped or changed container updates the proxies within about one second.

## Settings

Fill in "Docker Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.docker]
enabled = true
endpoint = "unix:///var/run/docker.sock"
network = "proxy"
exposed_by_default = false
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `endpoint` | `unix://<path>`, `tcp://<host>:<port>`, `http://<host>:<port>` or `https://<host>:<port>`. The default is `unix:///var/run/docker.sock`. Windows does not support `unix://`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` endpoint. A system root certificate or a root certificate in r3v3rs3 must sign the server certificate. |
| `network` | The Docker network of the upstream addresses. Leave it empty when each container is on one network. |
| `exposed_by_default` | `true` reads every container that has `r3v3rs3.` labels. `false` reads only the containers with `r3v3rs3.enable=true`. |

A change of the settings restarts the provider without a server restart. A certificate that the provider uses cannot be deleted.

## Containers

- The upstream address of a container is its IP address on the selected network. A container with the `host` network mode uses `127.0.0.1`.
- A container whose health check fails is skipped and shown as an issue. A container whose health check has not passed yet is skipped.
- The replicas of a Compose service share their proxies. Each replica adds its servers to the routes of the proxy, so the proxy balances the load across the replicas. The replicas must define the same number of routes.
- The key of a proxy is the Compose project, the Compose service and the proxy name. A container outside Compose uses its container name.
- r3v3rs3 reads the container list again every five minutes without events.

## Compose Example

```yaml
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks: [proxy]
    ports:
      - 80:80
      - 127.0.0.1:46492:46492

  whoami:
    image: traefik/whoami
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.whoami.ports: http
      r3v3rs3.http.whoami.vhosts: whoami.example.com
      r3v3rs3.http.whoami.port: "80"

networks:
  proxy:
    name: proxy

volumes:
  r3v3rs3-config:
```

Add a port with the name `http` in "Ports", enable the provider with the network `proxy`, and start the stack. Compose adds the project name to a network name unless the network sets `name`.

> Access to the Docker socket gives full control of the Docker host, which equals root access. The `:ro` option applies only to the socket file and does not make the API read-only. When the admin panel or the host is reachable from untrusted networks, connect r3v3rs3 to a Docker socket proxy that allows only `GET /containers/json` and `GET /events`.

# Kubernetes

The Kubernetes provider reads the `networking.k8s.io/v1` Ingress resources, the `r3v3rs3.io/v1` R3v3rs3Proxy resources, the Services and EndpointSlices of their backends, and the TLS secrets of the Ingress resources. It follows the changes with watch streams, so a changed resource, a new ready pod or a renewed secret updates the proxies within about one second.

The `deploy/kubernetes` directory holds the custom resource definition (`crd.yaml`), the service account with its ClusterRole (`rbac.yaml`) and an example Deployment (`deployment.yaml`).

## Settings

Fill in "Kubernetes Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.kubernetes]
enabled = true
kubeconfig = ""
namespaces = []
ingress = true
crd = true
ingress_class = "r3v3rs3"
ports = ["http"]
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `kubeconfig` | The path of a kubeconfig file. Leave it empty to use `KUBECONFIG` or `~/.kube/config`, or the service account of the pod inside a cluster. |
| `namespaces` | The namespaces to read. Leave it empty to read every namespace. |
| `ingress` | Reads the Ingress resources. The default is `true`. |
| `crd` | Reads the R3v3rs3Proxy resources. The default is `false`. Install the custom resource definition first, because the provider cannot watch a resource that the cluster does not know. The provider needs `ingress` or `crd`. |
| `ingress_class` | r3v3rs3 reads only the Ingress resources of this class. The class comes from `spec.ingressClassName` or the `kubernetes.io/ingress.class` annotation. Leave it empty to read every Ingress. |
| `ports` | The port names or ids of an Ingress without the `r3v3rs3.io/ports` annotation. |

A change of the settings restarts the provider without a server restart.

## Ingress Resources

- Each host of an Ingress becomes one HTTP proxy with the host as its virtual host. Each path of the host becomes a route. The name of the proxy is `<namespace>/<name> <host>`.
- The rules without a host and `spec.defaultBackend` become one proxy without a virtual host. The requests that no other proxy on the port matches reach this proxy.
- The `Prefix` and `ImplementationSpecific` path types match the path as a prefix. r3v3rs3 has no exact path match, so a path with the `Exact` type is an issue and does not become a route.
- The servers of a route are the ready endpoints of the Service port in the EndpointSlices of the Service. A backend selects the Service port by `port.number` or `port.name`. An endpoint without the `ready` condition is ready.
- A Service port with the name `https` or the `appProtocol` `https` uses HTTPS to the endpoints.
- A missing Service, a missing port or a Service without ready endpoints is an issue. The route stays without servers, so its requests receive an error and do not reach another route.
- Only a Service backend is available. A `resource` backend is an issue.

## Annotations

An `r3v3rs3.io/<field>` annotation sets a field of the proxies of the Ingress. The fields and values are the fields of an HTTP proxy in [Labels](@/discovery.md#labels) without the `r3v3rs3.http.<name>.` prefix.

| Annotation | Meaning |
|---|---|
| `r3v3rs3.io/ports` | The port names or ids of the proxies. It overrides the `ports` setting. An Ingress without this annotation and without the setting is an issue. |
| `r3v3rs3.io/name` | The name of the proxies. |
| `r3v3rs3.io/acme` | The ACME entry that orders a certificate for each host. See [ACME Certificates](@/discovery.md#acme-certificates). |
| `r3v3rs3.io/<field>` | Any other field, for example `r3v3rs3.io/rate_limit.requests` or `r3v3rs3.io/headers.response.0.name`. |

The rules of the Ingress set `routes` and `vhosts`, so an annotation for `routes`, `vhosts`, `port` or `scheme` is an issue.

## TLS Secrets

- r3v3rs3 reads the `kubernetes.io/tls` secrets that the `spec.tls` of a selected Ingress names, and adds each one to the certificate list as a server certificate. A TLS port selects the certificate by the server name, as with an uploaded certificate.
- r3v3rs3 does not save these certificates. The certificate list shows the provider, and a certificate of a secret cannot be deleted. When the Ingress or the secret is deleted, the certificate is removed.
- A missing secret, or a secret with an invalid certificate or private key, is an issue.

## R3v3rs3Proxy Resources

An R3v3rs3Proxy resource defines one proxy with the fields of the proxy model, so it can define a TCP or a UDP proxy and set every field. Install the definition, then enable `crd`:

```bash
kubectl apply -f deploy/kubernetes/crd.yaml
```

- `spec.protocol` is `http`, `tcp` or `udp`. The default is `http`.
- The other fields of `spec` are the fields of a proxy in [Labels](@/discovery.md#labels), for example `ports`, `name`, `active`, `acme`, `vhosts` and `routes`. A list is a YAML list, and a number or a boolean is a YAML value.
- `ports` is required unless the `ports` setting is set. The default name is `<namespace>/<name>`.
- `service` with `name` and `port` names a Service in the namespace of the resource. `port` is the number or the name of a Service port. In a route of an HTTP proxy, the ready endpoints of the Service become the servers of the route. In a TCP or UDP proxy, they become the upstream servers. `service` cannot be used together with `servers` or `upstream_servers`.
- A route whose Service has no ready endpoint stays without servers and is an issue, as with an Ingress.
- The resource has no address, so `port` and `scheme` are not available.
- A key inside `spec` cannot contain a `.`.
- `kubectl get rproxy` lists the resources.

```yaml
apiVersion: r3v3rs3.io/v1
kind: R3v3rs3Proxy
metadata:
  name: whoami
  namespace: default
spec:
  ports: [https]
  vhosts: [whoami.example.com]
  rate_limit:
    requests: 100
    per: minute
  routes:
    - path: /
      service:
        name: whoami
        port: http
---
apiVersion: r3v3rs3.io/v1
kind: R3v3rs3Proxy
metadata:
  name: postgres
  namespace: default
spec:
  protocol: tcp
  ports: [postgres]
  service:
    name: postgres
    port: 5432
```

## RBAC

The provider lists and watches five resources. `deploy/kubernetes/rbac.yaml` holds these objects. Grant the service account of r3v3rs3 a ClusterRole, or a Role in each namespace of `namespaces`:

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: r3v3rs3
  namespace: r3v3rs3
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: r3v3rs3
rules:
  - apiGroups: ["networking.k8s.io"]
    resources: ["ingresses"]
    verbs: ["get", "list", "watch"]
  - apiGroups: [""]
    resources: ["services", "secrets"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["discovery.k8s.io"]
    resources: ["endpointslices"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["r3v3rs3.io"]
    resources: ["r3v3rs3proxies"]
    verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: r3v3rs3
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: r3v3rs3
subjects:
  - kind: ServiceAccount
    name: r3v3rs3
    namespace: r3v3rs3
```

The provider reads only the secrets of the type `kubernetes.io/tls`, but Kubernetes RBAC cannot limit `list` to a type. The role therefore allows r3v3rs3 to read every secret in its namespaces. Use `namespaces` and a Role in each namespace to limit this access.

## Ingress Example

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: whoami
  namespace: default
  annotations:
    r3v3rs3.io/ports: https
    r3v3rs3.io/headers.response.0.action: set
    r3v3rs3.io/headers.response.0.name: X-Frame-Options
    r3v3rs3.io/headers.response.0.value: DENY
spec:
  ingressClassName: r3v3rs3
  tls:
    - hosts: [whoami.example.com]
      secretName: whoami-tls
  rules:
    - host: whoami.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: whoami
                port:
                  name: http
```

Add a TLS port with the name `https` in "Ports". r3v3rs3 must reach the pod addresses, so run it inside the cluster or on a node of the cluster.

# Consul

The Consul provider reads the `r3v3rs3.*` tags of the services in the catalog and the keys under a prefix in the key-value store. It follows the changes with blocking queries, so a registered, removed or failing service instance and a changed key update the proxies within about one second.

## Settings

Fill in "Consul Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.consul]
enabled = true
address = "http://127.0.0.1:8500"
token = "<ACL token>"
catalog = true
kv = true
prefix = "r3v3rs3"
exposed_by_default = false
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `address` | The HTTP API of a Consul agent: `http://<host>:<port>`, `https://<host>:<port>` or `unix://<path>`. The default is `http://127.0.0.1:8500`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` address. |
| `token` | The ACL token. The catalog needs `service:read` and `node:read`, and the key-value store needs `key:read` on the prefix. |
| `datacenter` | The datacenter to read. Leave it empty to read the datacenter of the agent. |
| `catalog` | Reads the tags of the services in the catalog. The default is `true`. |
| `kv` | Reads the keys under `prefix`. The default is `true`. |
| `prefix` | The key prefix. The default is `r3v3rs3`. |
| `exposed_by_default` | `true` reads every service that has `r3v3rs3.` tags. `false` reads only the services with the `r3v3rs3.enable=true` tag. |

The admin API does not return the token. `GET /api/config` returns `token_set: true` when a token is saved. A `PUT /api/config` without `token` keeps the saved token, and `"token": ""` removes it. `config.toml` holds the token as plain text, so allow only the r3v3rs3 user to read the config directory.

## Catalog Services

- A tag has the form `<key>=<value>`, for example `r3v3rs3.http.app.ports=https`. A `r3v3rs3.` tag without `=` is an issue.
- r3v3rs3 reads only the instances that pass their health checks. A selected service without a passing instance is an issue, and its proxies are removed.
- The upstream address of an instance is the service address. An instance without a service address uses the node address.
- A proxy without `port`, `routes` and `upstream_servers` uses the port of the service. A route without `port` and `servers` also uses the port of the service.
- The instances of a service share its proxies. Each instance adds its servers to the routes of the proxy, so the proxy balances the load across the instances.

Register a service with the Consul agent, for example with `consul services register whoami.json`:

```json
{
  "Service": {
    "Name": "whoami",
    "Port": 8080,
    "Tags": [
      "r3v3rs3.enable=true",
      "r3v3rs3.http.whoami.ports=http",
      "r3v3rs3.http.whoami.vhosts=whoami.example.com"
    ],
    "Check": { "HTTP": "http://localhost:8080/", "Interval": "10s" }
  }
}
```

## Key-Value Store

- A key under the prefix is a label. With the prefix `r3v3rs3`, the key `r3v3rs3/http/app/ports` is the label `r3v3rs3.http.app.ports`.
- r3v3rs3 reads every proxy under the prefix, so the keys do not need `r3v3rs3.enable`.
- A key has no address, so `port` is not available. Use `routes.<n>.servers.<n>.url` or `upstream_servers.<n>.addr`.
- A part of a key cannot contain a `.`. Such a key is an issue.

```bash
consul kv put r3v3rs3/http/app/ports https
consul kv put r3v3rs3/http/app/vhosts app.example.com
consul kv put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```

# etcd

The etcd provider reads the keys under a prefix through the etcd v3 HTTP API. It follows the changes with a watch stream, so a changed key updates the proxies within about one second.

## Settings

Fill in "etcd Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<password>"
prefix = "r3v3rs3"
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `endpoints` | The HTTP API addresses of the cluster members: `http://<host>:<port>`, `https://<host>:<port>` or `unix://<path>`. When a connection fails, r3v3rs3 connects to the next address. The default is `http://127.0.0.1:2379`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` endpoint. |
| `username` | The user of etcd authentication. Leave it empty when authentication is off. |
| `password` | The password of the user. Set it together with `username`. |
| `prefix` | The key prefix. The default is `r3v3rs3`. |

The admin API does not return the password. `GET /api/config` returns `password_set: true` when a password is saved. A `PUT /api/config` without `password` keeps the saved password, and `"password": ""` removes it. `config.toml` holds the password as plain text, so allow only the r3v3rs3 user to read the config directory.

With authentication, r3v3rs3 requests a token with the user name and the password. When etcd rejects an expired token, r3v3rs3 requests a new token and sends the request again. The user needs a role with the read permission on the prefix:

```bash
etcdctl role add r3v3rs3-reader
etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
etcdctl user add r3v3rs3
etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

## Keys

- The keys use the layout of the Consul key-value store. With the prefix `r3v3rs3`, the key `r3v3rs3/http/app/ports` is the label `r3v3rs3.http.app.ports`.
- r3v3rs3 reads every proxy under the prefix, so the keys do not need `r3v3rs3.enable`.
- A key has no address, so `port` is not available. Use `routes.<n>.servers.<n>.url` or `upstream_servers.<n>.addr`.
- A part of a key cannot contain a `.`. Such a key is an issue.
- When etcd compacts the revision that the watch stream needs, r3v3rs3 reads every key again.

```bash
etcdctl put r3v3rs3/http/app/ports https
etcdctl put r3v3rs3/http/app/vhosts app.example.com
etcdctl put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```
