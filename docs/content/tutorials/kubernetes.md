+++
title = "r3v3rs3 on Kubernetes"
description = "Run r3v3rs3 in a cluster and define proxies with Ingress and R3v3rs3Proxy resources"
weight = 8
+++

# r3v3rs3 on Kubernetes

r3v3rs3 runs in a cluster as an ingress controller. It reads the Ingress resources of its class and the R3v3rs3Proxy custom resources, follows the ready endpoints of their Services, and updates the proxies within about one second.

This guide installs r3v3rs3, defines one proxy with an Ingress and one with a custom resource, and checks both.

The `deploy/kubernetes` directory of the repository holds the three manifests that this guide applies: `rbac.yaml`, `crd.yaml` and `deployment.yaml`.

## Before You Start

- A cluster and a working `kubectl`.
- r3v3rs3 must reach the pod addresses, so run it inside the cluster, as this guide does.

## Step 1: Install the Service Account and the Definition

```bash
$ kubectl apply -f deploy/kubernetes/rbac.yaml
$ kubectl apply -f deploy/kubernetes/crd.yaml
$ kubectl wait --for condition=established --timeout=60s crd/r3v3rs3proxies.r3v3rs3.io
```

`rbac.yaml` creates the `r3v3rs3` namespace, the service account and a ClusterRole that may `get`, `list` and `watch` five resources: `ingresses`, `services`, `secrets`, `endpointslices` and `r3v3rs3proxies`.

The provider reads only the secrets of the type `kubernetes.io/tls`, but Kubernetes RBAC cannot limit `list` to a type, so the role allows every secret of the namespaces. To narrow this, set `namespaces` in Step 3 and replace the ClusterRole with a Role in each of them.

## Step 2: Run r3v3rs3

```bash
$ kubectl apply -f deploy/kubernetes/deployment.yaml
$ kubectl -n r3v3rs3 rollout status deployment/r3v3rs3
```

The manifest creates a 1 Gi PersistentVolumeClaim for `/root/.config/r3v3rs3`, a Deployment with one replica and the strategy `Recreate`, and a `LoadBalancer` Service for the ports 80 and 443.

The Deployment uses `serviceAccountName: r3v3rs3`, so the provider needs no kubeconfig inside the pod. The container image starts with `--webui 0.0.0.0:46492`.

Reach the admin panel without publishing it:

```bash
$ kubectl -n r3v3rs3 port-forward deployment/r3v3rs3 46492:46492
```

Open [http://localhost:46492/](http://localhost:46492/) and create the admin account:

```bash
$ kubectl -n r3v3rs3 exec deployment/r3v3rs3 -- r3v3rs3 add-user admin
```

## Step 3: Turn On the Kubernetes Provider

1. Click **Settings** in the menu.
2. Find **Kubernetes Service Discovery** and turn on **Read proxies from Kubernetes**.
3. Leave **Kubeconfig File** empty. The pod uses its service account.
4. Turn on **Read Ingress resources** and **Read R3v3rs3Proxy resources**.
5. Write `r3v3rs3` in **Ingress Class**.
6. Write `http` in **Default Ports**.
7. Save.

```toml
[discovery.kubernetes]
enabled = true
namespaces = []
ingress = true
crd = true
ingress_class = "r3v3rs3"
ports = ["http"]
```

**Read R3v3rs3Proxy resources** is off by default. Turn it on only after Step 1, because the provider cannot watch a resource that the cluster does not know.

**Default Ports** names the r3v3rs3 ports of an Ingress that carries no `r3v3rs3.io/ports` annotation. Bind those ports first: a discovery provider opens none. Add an HTTP port named `http` on 80 and a TLS port named `https` on 443, so the container ports of the Deployment match.

## Step 4: Define a Proxy with an Ingress

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: whoami
  namespace: default
  annotations:
    r3v3rs3.io/ports: https
    r3v3rs3.io/rate_limit.requests: "100"
    r3v3rs3.io/rate_limit.per: minute
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

- Each host becomes one HTTP proxy with that host as its virtual host, and each path becomes a route. The proxy name is `<namespace>/<name> <host>`.
- The servers of a route are the ready endpoints of the Service port, from the EndpointSlices of the Service. A new ready pod joins within about one second.
- `Prefix` and `ImplementationSpecific` match a path prefix. r3v3rs3 has no exact match, so `Exact` is an issue and the path becomes no route.
- Any other proxy field works as an `r3v3rs3.io/<field>` annotation. `routes`, `vhosts`, `port` and `scheme` come from the Ingress itself, so an annotation for them is an issue.
- r3v3rs3 adds the certificate of each `kubernetes.io/tls` secret of `spec.tls` to the certificate list. A TLS port selects it by the server name. r3v3rs3 does not save such a certificate and removes it with the Ingress or the secret.

## Step 5: Define a Proxy with a Custom Resource

An Ingress describes HTTP only. An R3v3rs3Proxy resource sets every field of the proxy model, TCP and UDP included.

```yaml
apiVersion: r3v3rs3.io/v1
kind: R3v3rs3Proxy
metadata:
  name: whoami
  namespace: default
spec:
  ports: [https]
  vhosts: [crd.example.com]
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

- `spec.protocol` is `http` (the default), `tcp` or `udp`.
- `service` names a Service of the same namespace. Its ready endpoints become the servers of an HTTP route or the upstream servers of a TCP or UDP proxy. It cannot be used together with `servers` or `upstream_servers`.
- The default proxy name is `<namespace>/<name>`.
- A key inside `spec` cannot hold a `.`.

```bash
$ kubectl get rproxy
NAME       AGE
whoami     20s
postgres   20s
```

## Step 6: Check the Result

The **Proxies** page lists both proxies with the source `kubernetes` and without an edit button, because the cluster owns them.

```bash
$ curl -b session.txt http://127.0.0.1:46492/api/discovery
[{"provider":"kubernetes","state":"running","proxies":2,"updated_at":1789643174}]
```

A resource that r3v3rs3 cannot use becomes an issue instead of a proxy. The usual ones:

| Issue | Cause |
|---|---|
| `port not found` | The Ingress names an r3v3rs3 port that does not exist. Bind it in **Ports**. |
| A route without servers | The Service is missing, the port name is wrong, or no pod is ready. Such a route answers with an error and does not fall through to another route. |
| A missing secret | `spec.tls` names a secret that does not exist, or its certificate or key is invalid. |

When you delete an Ingress, its proxy disappears. The custom resources keep theirs.

## What to Watch

- One replica. The Deployment uses the `Recreate` strategy and a ReadWriteOnce volume, because two replicas would hold two copies of the configuration. Use [cluster mode](@/tutorials/high-availability.md) for several replicas with one shared state.
- r3v3rs3 must reach the pod addresses. A r3v3rs3 outside the cluster reads the resources but cannot connect to the endpoints.
- The provider opens no port. Every Ingress and every custom resource names existing r3v3rs3 ports.

## Next Steps

- [Kubernetes](@/discovery.md#kubernetes): every setting, every annotation and the RBAC manifest.
- [Labels](@/discovery.md#labels): the field names that the annotations and `spec` use.
- [High Availability](@/tutorials/high-availability.md): several nodes with one shared state.
