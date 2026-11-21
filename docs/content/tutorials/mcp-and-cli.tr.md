+++
title = "MCP ve komut satırı istemcisi"
description = "MCP'yi açma, bir AI istemcisini bağlama, r3'ü kurma ve giriş yapma"
weight = 13
+++

# MCP ve komut satırı istemcisi

r3v3rs3, yönetim API'sini WebUI dışında iki yoldan daha sunar: `/api/mcp` altında Model Context Protocol tool'ları olarak ve `r3` komut satırı istemcisi üzerinden. İkisi de tarayıcıda OAuth 2.1 ile giriş yapar ve onayı veren hesabın rolüyle çalışır. Bu rehber MCP'yi açar, bir AI istemcisini bağlar, `r3`'ü kurar ve ilk komutları çalıştırır.

Örnekler, panelin genel adresi olarak `https://panel.example.com` kullanır. Siz WebUI'a hangi adresten ulaşıyorsanız onu kullanın.

## Adım 1: MCP'yi açma

MCP endpoint'i varsayılan olarak kapalıdır. **Ayarlar** sayfasındaki **MCP** kartına giderek **MCP endpoint'ini çalıştır** kutusunu işaretleyin. Değişiklik sunucu çalışırken uygulanır.

`r3`, `r3v3rs3-cli` client'ı olarak giriş yapar. Bu client'ı **MCP Client'ları** alanına ekleyin. Her satıra bir client yazılır: client id, ad ve redirect URI, virgülle ayrılmış:

```
r3v3rs3-cli, r3v3rs3 command line client, http://127.0.0.1:8765/callback
```

Loopback adresindeki bir redirect URI her portla eşleşir. Bu yüzden `r3` boş bir portu dinleyebilir. `config.toml` aynı ayarları tutar:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 command line client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

Claude Code veya Cursor gibi bir AI istemcisi için burada bir satır gerekmez. MCP açıldıktan sonra bu istemci kendini `/oauth/register` üzerinden kaydeder.

## Adım 2: Bir AI istemcisini bağlama

Claude Code, Codex ve OpenCode OAuth ile giriş yapar: her biri r3v3rs3'ün onay sayfasını tarayıcıda açar. Panel isterse giriş yapın, verdiğiniz kapsamları işaretleyin (durumu görmek için `read`, değiştirmek için `edit`) ve **Onayla** düğmesine basın. Sunucunun tool'ları bundan sonra kullanılabilir. Örnekler: `server_status`, `list_proxies` veya `deploy_app`.

### Claude Code

```bash
claude mcp add --transport http --scope user r3v3rs3 https://panel.example.com/api/mcp
```

Aynı sunucu, bir projenin `.mcp.json` dosyasında:

```json
{
  "mcpServers": {
    "r3v3rs3": {
      "type": "http",
      "url": "https://panel.example.com/api/mcp"
    }
  }
}
```

Ardından Claude Code'da `/mcp` komutunu çalıştırın, `r3v3rs3` sunucusunu seçin ve **Authenticate** seçeneğini seçin.

### Codex

```bash
codex mcp add r3v3rs3 --url https://panel.example.com/api/mcp
codex mcp login r3v3rs3
```

Komut `~/.codex/config.toml` dosyasına şu tabloyu yazar:

```toml
[mcp_servers.r3v3rs3]
url = "https://panel.example.com/api/mcp"
```

### OpenCode

```bash
opencode mcp add r3v3rs3 --url https://panel.example.com/api/mcp
opencode mcp auth r3v3rs3
```

Komut `opencode.json` dosyasına şu kaydı yazar (`--global` ise genel yapılandırmaya yazar):

```json
{
  "mcp": {
    "servers": {
      "r3v3rs3": {
        "type": "remote",
        "url": "https://panel.example.com/api/mcp"
      }
    }
  }
}
```

### Antigravity (agy)

agy OAuth akışını çalıştırmaz, sabit bir header gönderir. Ona hesabınızın bir API token'ını verin ([Adım 5](#adim-5-bir-script-icin-token) bir token oluşturur; `read` token'ı yalnız okur):

```bash
agy mcp add --header "Authorization: Bearer TOKEN" r3v3rs3 https://panel.example.com/api/mcp
```

Erişimi kesmek için token'ı `r3 <sunucu> token delete <id>` ile silin.

### Erişim

Hesabın rolü istemciyi yine sınırlar: viewer rolündeki bir hesap, `edit` kapsamıyla bile hiçbir şeyi değiştiremez. **Ayarlar** sayfasındaki **Aktif MCP Oturumları** listesi, giriş yapmış her istemciyi gösterir. Bir istemcinin oturumunu oradan kapatabilirsiniz.

## Adım 3: r3'ü kurma

Linux ve macOS'ta:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.sh | sh
```

Windows'ta (PowerShell):

```powershell
irm https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.ps1 | iex
```

Script en son sürümü indirir ve sha256 özetini doğrular. Ardından `r3`'ü `/usr/local/bin` dizinine kurar (`INSTALL_DIR` bu dizini değiştirir). Windows'ta dosya `%LOCALAPPDATA%\Programs\r3` dizinine kurulur; bu dizini `PATH` değişkeninize ekleyin. Daha sonra `r3 update`, binary'yi en son sürümle değiştirir.

## Adım 4: Giriş yapma ve ilk komutlar

Her komut önce panelin adresini alır:

```bash
r3 https://panel.example.com login
```

`login` onay sayfasını tarayıcıda açar ve onayı bekler. Ardından bu sunucunun token'larını kullanıcınızın yapılandırma dizinine kaydeder (`r3v3rs3-cli/<host>.json`, yalnız sizin okuyabildiğiniz bir dosya). `r3` access token'ı kendisi yeniler. Bu yüzden bir kez giriş yapmanız yeterlidir.

```bash
r3 https://panel.example.com platform get-app-info
r3 https://panel.example.com proxy list
r3 https://panel.example.com app list
```

`r3 --help` komut gruplarını listeler. `r3 <sunucu> <grup> --help` ise bir grubun komutlarını listeler. Her komut, API işleminin adını taşır: `deploy_app` işlemi `r3 <sunucu> app deploy <id>` komutudur.

## Adım 5: Bir script için token

Bir cron işi veya bir CI runner'ının tarayıcısı yoktur. Giriş yapmışken bir API token'ı oluşturun:

```bash
r3 https://panel.example.com token create --name ci-deploy --scope edit
```

Yanıt token'ı içerir. Token'ı işe `R3V3RS3_TOKEN` olarak verin veya `--token` ile iletin:

```bash
R3V3RS3_TOKEN=... r3 https://panel.example.com app deploy <id>
```

`r3 <sunucu> token list` hesabınızın token'larını gösterir. `r3 <sunucu> token delete <id>` bir token'ı siler.

## Sonraki adımlar

- [Yapılandırma başvurusu](@/reference/mcp-and-cli.tr.md) her MCP tool'unu listeler ve OAuth akışını ayrıntılı anlatır.
- [r3v3rs3'ü script ile yönetme](@/tutorials/admin-api.tr.md) aynı API'yi `curl` ile çağırır.
- [Ekip için hesaplar](@/tutorials/team-accounts.tr.md) bir istemci için daha az yetkili bir hesap oluşturur.
