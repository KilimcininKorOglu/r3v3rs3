+++
title = "MCP and the Command Line Client"
description = "Turn on MCP, connect an AI client, install r3 and sign in"
weight = 13
+++

# MCP and the Command Line Client

r3v3rs3 serves its admin API in two more ways besides the WebUI: as Model Context Protocol tools at `/api/mcp`, and through the `r3` command line client. Both sign in with OAuth 2.1 in the browser, and both act with the role of the account that approves them. This guide turns on MCP, connects an AI client, installs `r3` and runs the first commands.

The examples use `https://panel.example.com`, the public URL of the panel. Use the address under which you reach the WebUI.

## Step 1: Turn On MCP

The MCP endpoint is off by default. Open **Settings**, go to the **MCP** card, and check **Serve the MCP endpoint**. The change applies while the server runs.

`r3` signs in as the client `r3v3rs3-cli`. Add it to **MCP Clients**, one client per line, as the client id, a name and the redirect URI, separated by commas:

```
r3v3rs3-cli, r3v3rs3 command line client, http://127.0.0.1:8765/callback
```

A loopback redirect URI matches any port, so `r3` can listen on a free port. `config.toml` holds the same settings:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 command line client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

An AI client such as Claude Code or Cursor does not need a line here: it registers itself through `/oauth/register` once MCP is on.

## Step 2: Connect an AI Client

Claude Code, Codex and OpenCode sign in with OAuth: each opens the consent page of r3v3rs3 in the browser. Sign in if the panel asks, check the scopes you grant (`read` to see the state, `edit` to change it) and choose **Approve**. The tools of the server are then available, for example `server_status`, `list_proxies` or `deploy_app`.

### Claude Code

```bash
claude mcp add --transport http --scope user r3v3rs3 https://panel.example.com/api/mcp
```

The same server in the `.mcp.json` file of a project:

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

Then run `/mcp` in Claude Code, select `r3v3rs3` and choose **Authenticate**.

### Codex

```bash
codex mcp add r3v3rs3 --url https://panel.example.com/api/mcp
codex mcp login r3v3rs3
```

The command writes this table into `~/.codex/config.toml`:

```toml
[mcp_servers.r3v3rs3]
url = "https://panel.example.com/api/mcp"
```

### OpenCode

```bash
opencode mcp add r3v3rs3 --url https://panel.example.com/api/mcp
opencode mcp auth r3v3rs3
```

The command writes this entry into `opencode.json` (`--global` writes the global config instead):

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

agy does not run the OAuth flow; it sends a fixed header. Give it an API token of your account ([Step 5](#step-5-a-token-for-a-script) creates one; a `read` token only reads):

```bash
agy mcp add --header "Authorization: Bearer TOKEN" r3v3rs3 https://panel.example.com/api/mcp
```

Delete the token with `r3 <server> token delete <id>` to cut the access.

### Access

The role of the account still limits the client: a viewer account cannot change anything, even with the `edit` scope. **Settings** → **Active MCP Sessions** lists every signed-in client and revokes it with **Revoke Session**.

## Step 3: Install r3

On Linux and macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.sh | sh
```

On Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.ps1 | iex
```

The script downloads the newest release, checks its sha256 digest and installs `r3` into `/usr/local/bin` (`INSTALL_DIR` changes the directory) or into `%LOCALAPPDATA%\Programs\r3` on Windows, which you add to your `PATH`. `r3 update` later replaces the binary with the newest release.

## Step 4: Sign In and Run the First Commands

Every command takes the address of the panel first:

```bash
r3 https://panel.example.com login
```

`login` opens the consent page in the browser, waits for the approval and stores the tokens of this server in the config directory of your user (`r3v3rs3-cli/<host>.json`, readable only by you). `r3` refreshes the access token by itself, so you sign in once.

```bash
r3 https://panel.example.com platform get-app-info
r3 https://panel.example.com proxy list
r3 https://panel.example.com app list
```

`r3 --help` lists the command groups, and `r3 <server> <group> --help` lists the commands of a group. Each command carries the name of its API operation: `deploy_app` is `r3 <server> app deploy <id>`.

## Step 5: A Token for a Script

A cron job or a CI runner has no browser. Create an API token while you are signed in:

```bash
r3 https://panel.example.com token create --name ci-deploy --scope edit
```

The response contains the token. Give it to the job as `R3V3RS3_TOKEN`, or pass it with `--token`:

```bash
R3V3RS3_TOKEN=... r3 https://panel.example.com app deploy <id>
```

`r3 <server> token list` shows the tokens of your account, and `r3 <server> token delete <id>` removes one.

## Next

- The [configuration reference](@/configuration.md#mcp-server) lists every MCP tool and explains the OAuth flow in detail.
- [Driving r3v3rs3 from a Script](@/tutorials/admin-api.md) calls the same API with `curl`.
- [Accounts for a Team](@/tutorials/team-accounts.md) creates an account with fewer rights for a client.
