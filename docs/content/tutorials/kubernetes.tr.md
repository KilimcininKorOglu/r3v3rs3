+++
title = "Kubernetes'te r3v3rs3"
description = "r3v3rs3'ü cluster içinde çalıştırın, proxy'leri Ingress ve R3v3rs3Proxy ile tanımlayın"
weight = 10
+++

# Kubernetes'te r3v3rs3

r3v3rs3 cluster içinde bir ingress controller olarak çalışır. Kendi class'ındaki Ingress kaynaklarını ve R3v3rs3Proxy custom resource'larını okur, Service'lerinin hazır endpoint'lerini izler ve proxy'leri yaklaşık bir saniye içinde günceller.

Bu rehber r3v3rs3'ü kurar, bir proxy'yi Ingress ile, bir proxy'yi custom resource ile tanımlar ve ikisini de kontrol eder.

Bu rehberin uyguladığı üç manifest repository'nin `deploy/kubernetes` dizinindedir: `rbac.yaml`, `crd.yaml` ve `deployment.yaml`.

## Başlamadan önce

- Bir cluster ve çalışan bir `kubectl`.
- r3v3rs3'ün pod adreslerine ulaşması gerekir, bu yüzden onu bu rehberdeki gibi cluster içinde çalıştırın.

## Adım 1: Service account'u ve kaynak tanımını kurun

```bash
$ kubectl apply -f deploy/kubernetes/rbac.yaml
$ kubectl apply -f deploy/kubernetes/crd.yaml
$ kubectl wait --for condition=established --timeout=60s crd/r3v3rs3proxies.r3v3rs3.io
```

`rbac.yaml` dosyası `r3v3rs3` namespace'ini, service account'u ve beş kaynakta `get`, `list`, `watch` yapabilen bir ClusterRole'ü oluşturur: `ingresses`, `services`, `secrets`, `endpointslices` ve `r3v3rs3proxies`.

Sağlayıcı yalnız `kubernetes.io/tls` türündeki secret'ları okur, ama Kubernetes RBAC `list` iznini türe göre sınırlayamaz. Bu yüzden role, namespace'lerdeki bütün secret'lara izin verir. Bunu daraltmak için Adım 3'te `namespaces` alanını doldurun ve ClusterRole yerine her namespace'te bir Role kullanın.

## Adım 2: r3v3rs3'ü çalıştırın

```bash
$ kubectl apply -f deploy/kubernetes/deployment.yaml
$ kubectl -n r3v3rs3 rollout status deployment/r3v3rs3
```

Manifest `/root/.config/r3v3rs3` için 1 Gi'lik bir PersistentVolumeClaim, tek replikalı ve `Recreate` stratejili bir Deployment ve 80 ile 443 portları için bir `LoadBalancer` Service oluşturur.

Deployment `serviceAccountName: r3v3rs3` kullanır, bu yüzden sağlayıcının pod içinde kubeconfig'e ihtiyacı yoktur. Container image `--webui 0.0.0.0:46492` ile başlar.

Panele, onu dışarı açmadan ulaşın:

```bash
$ kubectl -n r3v3rs3 port-forward deployment/r3v3rs3 46492:46492
```

[http://localhost:46492/](http://localhost:46492/) adresini açın ve admin hesabını oluşturun:

```bash
$ kubectl -n r3v3rs3 exec -it deployment/r3v3rs3 -- r3v3rs3 add-user admin
```

## Adım 3: Kubernetes sağlayıcısını açın

1. Menüde **Ayarlar** linkine tıklayın.
2. **Kubernetes Servis Keşfi** bölümünü bulun ve **Proxy'leri Kubernetes'ten oku** seçeneğini açın.
3. **Kubeconfig Dosyası** alanını boş bırakın. Pod kendi service account'unu kullanır.
4. **Ingress kaynaklarını oku** ve **R3v3rs3Proxy kaynaklarını oku** seçeneklerini açın.
5. **Ingress Class** alanına `r3v3rs3` yazın.
6. **Varsayılan Portlar** alanına `http` yazın.
7. Kaydedin.

```toml
[discovery.kubernetes]
enabled = true
namespaces = []
ingress = true
crd = true
ingress_class = "r3v3rs3"
ports = ["http"]
```

**R3v3rs3Proxy kaynaklarını oku** seçeneği varsayılan olarak kapalıdır. Onu yalnız Adım 1'den sonra açın, çünkü sağlayıcı cluster'ın tanımadığı bir kaynağı izleyemez.

**Varsayılan Portlar**, `r3v3rs3.io/ports` annotation'ı taşımayan bir Ingress'in r3v3rs3 portlarını yazar. Önce o portları bağlayın: servis keşfi sağlayıcısı hiç port açmaz. `http` adıyla 80 portunda bir HTTP portu, `https` adıyla 443 portunda bir TLS portu ekleyin. Böylece Deployment'ın container portlarıyla eşleşirler.

## Adım 4: Ingress ile bir proxy tanımlayın

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

- Her host, o host'u virtual host olarak taşıyan bir HTTP proxy olur. Her yol bir route olur. Proxy adı `<namespace>/<name> <host>` biçimindedir.
- Bir route'un sunucuları, Service portunun hazır endpoint'leridir. Bunlar Service'in EndpointSlice'larından gelir. Hazır olan yeni bir pod yaklaşık bir saniye içinde katılır.
- `Prefix` ve `ImplementationSpecific` yolu önek olarak eşler. r3v3rs3'te tam eşleşme yoktur, bu yüzden `Exact` sorun sayılır ve o yol route olmaz.
- Diğer bütün proxy alanları `r3v3rs3.io/<alan>` annotation'ı olarak çalışır. `routes`, `vhosts`, `port` ve `scheme` Ingress'in kendisinden gelir, bu yüzden onlar için yazılan bir annotation sorun sayılır.
- r3v3rs3, `spec.tls` alanındaki her `kubernetes.io/tls` secret'ının sertifikasını sertifika listesine ekler. TLS portu onu sunucu adıyla seçer. r3v3rs3 böyle bir sertifikayı saklamaz; Ingress veya secret silinince sertifikayı da kaldırır.

## Adım 5: Custom resource ile bir proxy tanımlayın

Ingress yalnız HTTP'yi anlatır. R3v3rs3Proxy kaynağı proxy modelinin bütün alanlarını, TCP ve UDP dahil, ayarlar.

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

- `spec.protocol` değeri `http` (varsayılan), `tcp` veya `udp` olur.
- `service`, aynı namespace'teki bir Service'i gösterir. Onun hazır endpoint'leri bir HTTP route'unun sunucuları veya bir TCP ya da UDP proxy'sinin upstream sunucuları olur. `servers` veya `upstream_servers` ile birlikte kullanılamaz.
- Varsayılan proxy adı `<namespace>/<name>` biçimindedir.
- `spec` içindeki bir key `.` taşıyamaz.

```bash
$ kubectl get rproxy
NAME       PROTOCOL   PORTS          AGE
postgres   tcp        ["postgres"]   20s
whoami                ["https"]      20s
```

## Adım 6: Sonucu kontrol edin

**Proxy'ler** sayfası iki proxy'yi de kaynağı **Kubernetes** olarak listeler ve düzenleme butonu göstermez, çünkü onların sahibi cluster'dır.

```bash
$ curl -b session.txt http://127.0.0.1:46492/api/discovery
[{"provider":"kubernetes","state":"running","proxies":2,"updated_at":1789643174}]
```

r3v3rs3'ün kullanamadığı bir kaynak proxy olmaz, sorun olarak görünür. Sık görülenler:

| Sorun | Nedeni |
|---|---|
| `port not found` | Ingress, var olmayan bir r3v3rs3 portunu yazıyordur. Portu **Portlar** sayfasında bağlayın. |
| Sunucusuz route | Service yoktur, port adı yanlıştır veya hazır pod yoktur. Böyle bir route hata döndürür ve istek başka bir route'a geçmez. |
| Eksik secret | `spec.tls`, var olmayan bir secret'ı gösteriyordur ya da sertifikası veya key'i geçersizdir. |

Bir Ingress'i sildiğinizde proxy'si kaybolur. Custom resource'lar proxy'lerini korur.

## Nelere dikkat etmeli

- Tek replika. Deployment `Recreate` stratejisini ve ReadWriteOnce bir volume'ü kullanır, çünkü iki replika yapılandırmanın iki kopyasını tutardı. Tek durum bilgisini paylaşan birkaç replika için [cluster modunu](@/tutorials/high-availability.tr.md) kullanın.
- r3v3rs3 pod adreslerine ulaşabilmelidir. Cluster dışındaki bir r3v3rs3 kaynakları okur, ama endpoint'lere bağlanamaz.
- Sağlayıcı hiç port açmaz. Her Ingress ve her custom resource var olan r3v3rs3 portlarını yazar.

## Sonraki adımlar

- [Kubernetes](@/discovery.tr.md#kubernetes): bütün ayarlar, bütün annotation'lar ve RBAC manifest'i.
- [Etiketler](@/discovery.tr.md#etiketler): annotation'ların ve `spec` alanlarının kullandığı alan adları.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir durum bilgisini paylaşan birkaç düğüm.
