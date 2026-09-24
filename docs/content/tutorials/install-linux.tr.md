+++
title = "Linux sunucuya kurulum"
description = "install.sh ile r3v3rs3'ü systemd servisi olarak kurun"
weight = 1
+++

# Linux sunucuya kurulum

`install.sh` release binary'sini indirir, sha256 özetini kontrol eder, admin hesabını oluşturur ve r3v3rs3'ü systemd servisi olarak çalıştırır. Tek bir komut kurar, aynı komut sonra yükseltir.

## Başlamadan önce

Script şunları ister:

- systemd çalışan bir Linux. Başka bir init sisteminde durur.
- `x86_64` veya `aarch64` mimarisi. Başkası için release binary'si yoktur.
- root ve şu komutlar: `curl`, `tar`, `xz`, `sha256sum`, `systemctl`.

## Adım 1: Script'i çalıştırın

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

Script iki soru sorar:

1. **Admin WebUI adresi.** Varsayılan `127.0.0.1:46492` değeridir, yani panel yalnız sunucunun kendisinde cevap verir. `0.0.0.0:46492` değerini yalnız o portu bir firewall veya bir VPN koruyorsa yazın.
2. **Admin kullanıcı adı**, sonra parola. Başarısız denemeden sonra tekrar sorar, toplam üç kez.

Çıktı bütün yolları yazar:

```text
r3v3rs3 v1.0.1 is running.

  WebUI:    http://127.0.0.1:46492/
  Binary:   /usr/local/bin/r3v3rs3
  Config:   /etc/r3v3rs3
  Logs:     /var/log/r3v3rs3 and journalctl -u r3v3rs3
  Service:  systemctl status|restart|stop r3v3rs3
  Add user: /usr/local/bin/r3v3rs3 add-user --config-dir /etc/r3v3rs3 <name>
```

Soru sormadan kurmak için iki seçenek de vardır:

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh \
    | sudo bash -s -- --version 1.0.1 --webui 0.0.0.0:46492
```

Terminal olmadan, pipe içinden çalışan bir script hesap oluşturmaz. Onun yerine komutu yazar:

```bash
$ sudo r3v3rs3 add-user --config-dir /etc/r3v3rs3 admin
```

## Adım 2: Servisi kontrol edin

```bash
$ systemctl is-enabled r3v3rs3
enabled
$ systemctl is-active r3v3rs3
active
$ curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:46492/
200
```

Script `/etc/systemd/system/r3v3rs3.service` dosyasını yazar:

```ini
[Service]
Type=simple
Environment=R3V3RS3_CONFIG_DIR=/etc/r3v3rs3
Environment=R3V3RS3_LOG_DIR=/var/log/r3v3rs3
Environment=R3V3RS3_WEBUI=127.0.0.1:46492
ExecStart=/usr/local/bin/r3v3rs3 start
KillSignal=SIGINT
Restart=on-failure
RestartSec=5
LimitNOFILE=1048576
```

`KillSignal=SIGINT` satırı önemlidir: r3v3rs3 SIGINT ile düzgün kapanır. `LimitNOFILE=1048576` file descriptor limitini yükseltir, çünkü her bağlantı bir tane ister.

Servis root olarak çalışır, bu yüzden r3v3rs3 1024 altındaki bir portu ek ayar olmadan bağlar.

## Adım 3: Paneli açın

Varsayılan adres yalnız sunucuda cevap verir. Portu dışarı açmak yerine SSH ile ulaşın:

```bash
$ ssh -L 46492:127.0.0.1:46492 kullanici@sunucunuz
```

[http://localhost:46492/](http://localhost:46492/) adresini açın ve Adım 1'deki hesapla giriş yapın. [Başlangıç](@/tutorials/getting-started.tr.md) rehberi ilk port ve ilk proxy ile devam eder.

## Dosyalar nerede

| Yol | İçerik |
|---|---|
| `/usr/local/bin/r3v3rs3` | Binary. |
| `/etc/r3v3rs3` | `accounts.toml`, `proxies.toml`, `ports.toml`, `access_lists.toml`, `acme.toml`, `config.toml` ve sertifikalar. Dizinin modu `0700` değeridir. |
| `/var/log/r3v3rs3` | Denetim kaydını tutan `log.db`. Dizinin modu `0750` değeridir. |

Sunucu log'unu `journalctl -u r3v3rs3` gösterir. `/etc/r3v3rs3/acme.toml` dosyası ACME hesap key'lerini ve DNS sağlayıcılarının kimlik bilgilerini düz metin tutar, bu yüzden bütün dizini yedekleyin ve yedeği koruyun.

## Yükseltme

Aynı komutu tekrar çalıştırın:

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

- Script binary'yi yeniden adlandırarak değiştirir, çünkü çalışan servis eski dosyayı açık tutar.
- Yapılandırmayı ve hesapları korur. İkinci bir hesap oluşturmaz.
- WebUI sorusunda varsayılan olarak kurulu servisin adresini sunar, yani Enter adresi korur.
- Servisi yeniden başlatır ve WebUI cevap verene kadar bekler. WebUI 30 saniye sessiz kalırsa `systemctl status` çıktısını ve son 50 journal satırını yazar, sonra durur.

Bir filoyu adım adım yükseltirken sürümü `--version 1.0.1` ile sabitleyin.

## Kaldırma

```bash
$ sudo systemctl disable --now r3v3rs3
$ sudo rm /etc/systemd/system/r3v3rs3.service /usr/local/bin/r3v3rs3
$ sudo systemctl daemon-reload
```

Bu adımlar `/etc/r3v3rs3` ve `/var/log/r3v3rs3` dizinlerini bırakır. Onları yalnız hiçbir sertifikayı ve hesabı saklamayacaksanız silin.

## Diğer kurulum yolları

- [Docker](@/tutorials/install-docker.tr.md): iki volume ile tek container.
- `cargo binstall r3v3rs3` veya `cargo install r3v3rs3`: crates.io paketi WebUI'yi içinde taşır, bu yüzden trunk istemez.
- [Releases sayfası](https://github.com/KilimcininKorOglu/r3v3rs3/releases): elle kurulum için arşiv. Binary'yi `PATH` içine koyun ve kendi unit dosyanızı yazın.

## Sonraki adımlar

- [Başlangıç](@/tutorials/getting-started.tr.md): ilk port ve ilk proxy.
- [Yapılandırma dosyaları](@/configuration.tr.md#yapilandirma-dosyalari): config dizinindeki her dosyanın içeriği.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir durum bilgisini paylaşan birkaç sunucu.
