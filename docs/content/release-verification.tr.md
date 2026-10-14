---
title: "Release doğrulama"
weight: 160
---

Her release bir `SHA256SUMS` dosyası ve onun minisign imzası olan
`SHA256SUMS.minisig` ile çıkar. Public key, repoda `dist/minisign.pub`
yolunda durur.

## İndirdiğiniz dosyayı doğrulayın

`minisign` kurun (örneğin `apt install minisign` veya
`brew install minisign`), release sayfasından arşivi, `SHA256SUMS` ve
`SHA256SUMS.minisig` dosyalarını indirin ve public key'i yanlarına kaydedin:

```console
$ curl -LO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/dist/minisign.pub
```

Sonra checksum'ları doğrulayın:

```console
$ minisign -Vm SHA256SUMS -p minisign.pub
Signature and comment signature verified
```

İmza uyuşmadığında `minisign` sıfırdan farklı bir status ile çıkar. İmza
iyiyse arşivi checksum'larla karşılaştırın:

```console
$ sha256sum --check --ignore-missing SHA256SUMS
r3v3rs3-x86_64-unknown-linux-gnu.tar.xz: OK
```

Docker image'larının ayrı bir imzası yoktur. `docker pull` çıktısındaki
digest, registry'nin servis ettiği image baytlarını tanımlar.
