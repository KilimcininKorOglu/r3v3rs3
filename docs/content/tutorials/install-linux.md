+++
title = "Installing on a Linux Server"
description = "Install r3v3rs3 as a systemd service with install.sh"
weight = 1
+++

# Installing on a Linux Server

`install.sh` downloads the release binary, checks its sha256 digest, creates the admin account and runs r3v3rs3 as a systemd service. One command installs it, and the same command upgrades it later.

## Before You Start

The script needs:

- Linux with a running systemd. It stops on another init system.
- The `x86_64` or the `aarch64` architecture. No release binary exists for another one.
- root, and the commands `curl`, `tar`, `xz`, `sha256sum` and `systemctl`.

## Step 1: Run the Script

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

The script asks two questions:

1. **Admin WebUI address.** The default is `127.0.0.1:46492`, so the panel answers only on the server itself. Write `0.0.0.0:46492` only when a firewall or a VPN protects that port.
2. **Admin user name**, and then the password. It asks again after a failed try, three times in all.

The output names every path:

```text
r3v3rs3 v1.0.1 is running.

  WebUI:    http://127.0.0.1:46492/
  Binary:   /usr/local/bin/r3v3rs3
  Config:   /etc/r3v3rs3
  Logs:     /var/log/r3v3rs3 and journalctl -u r3v3rs3
  Service:  systemctl status|restart|stop r3v3rs3
  Add user: /usr/local/bin/r3v3rs3 add-user --config-dir /etc/r3v3rs3 <name>
```

Both options also work without a question, for an unattended install:

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh \
    | sudo bash -s -- --version 1.0.1 --webui 0.0.0.0:46492
```

A script that runs through a pipe without a terminal creates no account. It prints the command instead:

```bash
$ sudo r3v3rs3 add-user --config-dir /etc/r3v3rs3 admin
```

## Step 2: Check the Service

```bash
$ systemctl is-enabled r3v3rs3
enabled
$ systemctl is-active r3v3rs3
active
$ curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:46492/
200
```

The script writes `/etc/systemd/system/r3v3rs3.service`:

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

`KillSignal=SIGINT` matters: r3v3rs3 shuts down gracefully on SIGINT. `LimitNOFILE=1048576` raises the file descriptor limit, because each connection needs one.

The service runs as root, so r3v3rs3 binds a port below 1024 without another setting.

## Step 3: Open the Panel

The default address answers only on the server. Reach it through SSH instead of publishing the port:

```bash
$ ssh -L 46492:127.0.0.1:46492 user@your-server
```

Open [http://localhost:46492/](http://localhost:46492/) and sign in with the account of Step 1. [Getting Started](@/tutorials/getting-started.md) continues with the first port and the first proxy.

## Where the Files Are

| Path | Content |
|---|---|
| `/usr/local/bin/r3v3rs3` | The binary. |
| `/etc/r3v3rs3` | `accounts.toml`, `proxies.toml`, `ports.toml`, `access_lists.toml`, `acme.toml`, `config.toml` and the certificates. The directory has the mode `0700`. |
| `/var/log/r3v3rs3` | `log.db`, which holds the audit log. The directory has the mode `0750`. |

`journalctl -u r3v3rs3` shows the server log. `/etc/r3v3rs3/acme.toml` holds the ACME account keys and the DNS provider credentials in plain text, so back up the whole directory and protect the backup.

## Upgrade

Run the same command again:

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

- The script replaces the binary with a rename, because the running service holds the old file open.
- It keeps the configuration and the accounts. It creates no second account.
- The WebUI question offers the address of the installed service as its default, so pressing Enter keeps it.
- It restarts the service and waits until the WebUI answers. When the WebUI stays silent for 30 seconds, it prints `systemctl status` and the last 50 journal lines, then stops.

Pin a release with `--version 1.0.1` when you upgrade a fleet in steps.

## Remove

```bash
$ sudo systemctl disable --now r3v3rs3
$ sudo rm /etc/systemd/system/r3v3rs3.service /usr/local/bin/r3v3rs3
$ sudo systemctl daemon-reload
```

This leaves `/etc/r3v3rs3` and `/var/log/r3v3rs3`. Delete them only when you keep no certificate and no account.

## Install an Agent

An [agent target](@/platform.md#agent-targets) of the deployment platform runs apps on another server. On that server Docker Engine must run. Add the target on the **Targets** page of the master, then run the command that the page shows:

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh \
    | sudo bash -s -- --agent --master master.example.com:9443 --token <token>
```

The script installs the same binary and runs `r3v3rs3 agent` as the systemd service `r3v3rs3-agent`:

```ini
[Service]
Type=simple
Environment=R3V3RS3_AGENT_MASTER=master.example.com:9443
Environment=R3V3RS3_AGENT_DATA_DIR=/var/lib/r3v3rs3-agent
EnvironmentFile=-/var/lib/r3v3rs3-agent/token.env
ExecStart=/usr/local/bin/r3v3rs3 agent
KillSignal=SIGINT
Restart=always
RestartSec=5
```

The token stays in `token.env` (mode `0600`) only until the agent enrolls. The script waits up to 30 seconds for the enrollment, then deletes the file. When the enrollment fails, it prints the last 50 journal lines and stops. The **Targets** page shows the target as **Online** when the agent is connected.

To upgrade the agent, run the script again with `--agent` and without a token. To enroll the server again, create a **New token** on the **Targets** page and pass it with `--token`. To remove the agent:

```bash
$ sudo systemctl disable --now r3v3rs3-agent
$ sudo rm /etc/systemd/system/r3v3rs3-agent.service
$ sudo systemctl daemon-reload
```

`/var/lib/r3v3rs3-agent` holds the key of the agent and the files of its Compose apps.

## Other Install Methods

- [Docker](@/tutorials/install-docker.md): one container with two volumes.
- `cargo binstall r3v3rs3` or `cargo install r3v3rs3`: the crates.io package ships the WebUI, so it needs no trunk.
- The [releases page](https://github.com/KilimcininKorOglu/r3v3rs3/releases): the archive for a manual install. Put the binary in your `PATH` and write your own unit file.

## Next Steps

- [Getting Started](@/tutorials/getting-started.md): the first port and the first proxy.
- [Configuration Files](@/configuration.md#configuration-files): what each file in the config directory holds.
- [High Availability](@/tutorials/high-availability.md): several servers with one shared state.
