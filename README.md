# tack

Todo tree for the terminal, the browser and coding agents.

![tack demo](assets/demo.gif)

```console
$ tack add work/boring_job/fix login
todo  /work/boring_job/fix_login

$ tack ls /work
todo  /work
todo  /work/boring_job
todo  /work/boring_job/fix_login
```

- **paths, not lists.** Items nest like files. Missing parents are created on the way, like `mkdir -p`
- **fast.** The tree lives in memory as a hash map of nodes. SQLite only sees writes
- **no setup.** Two static binaries, no runtime, no accounts. State is one SQLite file
- **three ways in.** Web UI, CLI and MCP for agents, one server, one query language
- **small.** The web UI is one 33 KB HTML file: no framework, no build step

## Install

Linux, x86_64 and aarch64.

**Server.** Installs `tackd` and runs it as a systemd user service on port 15470:

```sh
curl -fsSL https://raw.githubusercontent.com/ueco-jb/tack/master/install.sh | sh -s -- --server
```

**Client.** Installs `tack`:

```sh
curl -fsSL https://raw.githubusercontent.com/ueco-jb/tack/master/install.sh | sh
```

Binaries go to `~/.local/bin`. Rerun to upgrade.

## Server

```
tackd [--addr 0.0.0.0:15470] [--db <file>] [--log <file>] [--allow-host <name>]
```

- **database:** `~/.local/share/tack/tack.db`
- **request log:** `~/.local/state/tack/requests.log`, one JSON line per change
- **service:** `systemctl --user status tackd`, logs in `journalctl --user -u tackd`

**Access.** Every machine on the network is allowed until you add one to the allowlist. From then on, only listed machines and the server itself get in. Run on the server:

| Command | Does |
|---|---|
| `tackd allow` | list |
| `tackd allow 192.168.1.20` | add a machine |
| `tackd deny 192.168.1.20` | remove; an empty list allows everyone again |

**Host names.** `tackd` only answers requests addressed to its IP, `localhost`, its hostname or `<hostname>.local`, which blocks DNS rebinding from malicious web pages. Reaching it under another name, like `tack.lan`? Start it with `--allow-host tack.lan`.

**Boot.** User services stop at logout unless lingering is on. The installer enables it, or tells you to run `sudo loginctl enable-linger $USER`.

## Terminal

The first `tack` in a terminal asks for the server once and saves it to `~/.config/tack/server`:

```console
$ tack ls
tack server [http://localhost:15470]: myhost
```

`myhost` expands to `http://myhost:15470`. Change it with `tack server <url>`, override one call with `--url`.

| Command | Does |
|---|---|
| `tack ls [query]` | list matching items |
| `tack show <item>` | one item with its body |
| `tack add <path>` | add; `-b` body, `-t` tag, `-d` due date, `-s` status |
| `tack done <item>` | mark done; `-f` with open children |
| `tack mv <item> <path>` | move or rename |
| `tack set <item>` | change status, body, tags, due date, archive |
| `tack rm <item>` | delete; `-r` with children |

```console
$ tack ls /coding
todo   /coding/project-x
doing  /coding/project-x/milestone  +api +bug  2026-12-01
done   /coding/project-x/release
```

## Search

Same query language in the web UI, `tack ls` and MCP.

| Query | Matches |
|---|---|
| `login` | paths containing the word |
| `/coding/project-x` | the path and everything under it |
| `-login` | paths without the word |
| `+bug`, `tag:bug` | items tagged `bug` |
| `todo`, `doing`, `done` | items with that status |
| `is:open`, `is:done` | open or done items |
| `is:archived`, `is:all` | archived items, or everything |

Terms combine: `/coding +bug todo`. Without `is:`, archived items are hidden.

## Agents

`tackd` serves MCP over HTTP at `/mcp`.

**omp**, in `~/.omp/agent/mcp.json`:

```json
{
  "mcpServers": {
    "tack": { "type": "http", "url": "http://myhost:15470/mcp" }
  }
}
```

**Claude Code**:

```sh
claude mcp add --scope user --transport http tack http://myhost:15470/mcp
```

Ask in plain words. The agent picks the path and tags:

```console
> add the login redirect bug to tack
todo   /work/boring_job/fix_login_redirect  +bug

> what's still open in project-x?
todo   /coding/project-x/milestone_2/rate_limit_login     +api
doing  /coding/project-x/milestone_2/migrate_users_table  +db

> users table is migrated, close it and the milestone if nothing else is left
done   /coding/project-x/milestone_2/migrate_users_table  +db
todo   /coding/project-x/milestone_2: rate_limit_login is still open
```

Tools: `search`, `get`, `add`, `update`, `delete`, `labels`.

## Uninstall

**Server:**

```sh
systemctl --user disable --now tackd
rm ~/.local/bin/tackd ~/.config/systemd/user/tackd.service
```

The list stays in `~/.local/share/tack` and the request log in `~/.local/state/tack`. Delete those too to start over.

**Client:**

```sh
rm ~/.local/bin/tack
rm -r ~/.config/tack
```
