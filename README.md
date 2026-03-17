# S.A.M Mission Control

[![CI](https://github.com/tinybluedev/sam-mission-control/actions/workflows/ci.yml/badge.svg)](https://github.com/tinybluedev/sam-mission-control/actions/workflows/ci.yml)
[![Latest Release](https://img.shields.io/github/v/release/tinybluedev/sam-mission-control)](https://github.com/tinybluedev/sam-mission-control/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

A terminal-based fleet orchestration tool for managing distributed AI agents over SSH and Tailscale mesh networks. Built in Rust with [Ratatui](https://ratatui.rs).

![Dashboard](docs/screenshots/dashboard.png)

## Why S.A.M?

Modern AI deployments spread agents across many machines — home labs, VPS nodes, edge devices. Keeping track of which agents are alive, what tasks they're running, and sending them instructions without exposing ports is painful.

S.A.M (Systems Administration & Management) solves this by providing a **zero-exposure TUI** that tunnels everything through SSH and Tailscale. No open ports, no API keys flying over the internet, no separate web dashboard to secure. It talks to your agents the same way you would manually — over SSH — while keeping a persistent state in a shared MySQL database.

Use S.A.M when you:
- Run a self-hosted fleet of [OpenClaw](https://github.com/openclaw/openclaw) AI agents
- Need real-time health monitoring without a cloud service
- Want to chat with individual agents or broadcast tasks from a single terminal
- Value minimal attack surface and air-gap-compatible design

## Features

- **Fleet Dashboard** — Real-time agent status with SSH health probes
- **AI Chat** — Talk to any agent via OpenClaw HTTP API (SSH fallback when HTTP is blocked)
- **Task Board** — Create, assign, and track tasks across agents
- **Agent Detail** — Deep-dive into individual agent info and direct messaging
- **Multi-select** — Batch operations on groups of agents
- **8 Color Themes** — Standard, Noir, Paper, 1977, 2077, Matrix, Sunset, Arctic
- **Zero Network Exposure** — SSH + Unix socket only, no open ports

## Screenshots

### Splash Screen
![Splash](docs/screenshots/splash.png)

### Agent Detail & Chat
![Agent Detail](docs/screenshots/agent-detail.png)

### Task Board
![Task Board](docs/screenshots/task-board.png)

### Keybindings
![Help](docs/screenshots/help.png)

### Fleet Doctor
![Doctor](docs/screenshots/doctor.png)

### CLI Status
![Status](docs/screenshots/status-cli.png)

## Quick Start

```bash
curl -sSL https://raw.githubusercontent.com/tinybluedev/sam-mission-control/main/install.sh | bash
sam init
sam
```

## Commands

| Command | Description |
|---------|-------------|
| `sam` | Launch the TUI dashboard |
| `sam status` | Quick fleet status (non-interactive) |
| `sam doctor` | Diagnose fleet issues |
| `sam doctor --fix` | Auto-repair fleet issues |
| `sam init` | Interactive first-time setup |
| `sam onboard <ip>` | Provision a new agent |
| `sam deploy <agent> --file <path>` | Push files to agent workspace |
| `sam version` | Show version |

## Keybindings

| Key | Action |
|-----|--------|
| `Tab` | Switch focus: Fleet ↔ Chat |
| `Enter` | Open agent detail / send message |
| `j/k` or `↑/↓` | Navigate fleet list |
| `t` | Task board |
| `s` | Sort agents |
| `f` | Filter/search |
| `c` | Cycle color theme |
| `m` | Cycle dark/light mode (`auto` → `dark` → `light`) |
| `R` | Refresh all agents (SSH) |
| `Space` | Toggle select current agent |
| `a` / `A` | Select all / clear selection |
| `g` | Select all in current group filter |
| `Esc` | Clear selection |
| `r` | Restart gateway (selected) |
| `P` | Config push (selected) |
| `?` | Help |
| `q` | Quit |

## Requirements

- Rust 1.75+ (for building)
- MySQL/MariaDB database
- SSH access to fleet machines (key-based auth)
- [OpenClaw](https://github.com/openclaw/openclaw) on managed agents
- [Tailscale](https://tailscale.com) or [Headscale](https://github.com/juanfont/headscale) mesh (recommended)

## Configuration

Config file: `~/.config/sam/config.toml`

```toml
[database]
host = "10.0.0.2"
port = 3306
user = "sam"
name = "sam_fleet"

[self]
ip = "10.0.0.1"
```

Or use environment variables via `.env`:

```bash
SAM_DB_URL=mysql://user:pass@host:port/database
SAM_SELF_IP=10.0.0.1
```

## Agent Detection

S.A.M uses a **two-phase detection** strategy to determine each agent's status:

### Phase 1: Tailscale Connectivity Check
Before any SSH attempts, S.A.M runs `tailscale status --json` once per refresh cycle. This returns the online/offline state of every node in the Tailscale mesh instantly, without opening any connections.

- **Offline nodes are skipped immediately** — no SSH timeout waited (saves ~3s per offline agent)
- **Online nodes proceed to Phase 2**
- **Graceful degradation** — if the `tailscale` binary is unavailable or returns an error, S.A.M falls back to SSH-only probing with zero behavior change

### Phase 2: SSH Gateway Verification
For nodes that Tailscale reports as online, S.A.M verifies the OpenClaw gateway service via SSH:

- **Fast probe** (4 out of every 5 cycles): `ssh echo ok` — confirms SSH reachability and measures latency
- **Full probe** (every 5th cycle): SSHes in to collect OS, kernel version, OpenClaw version, CPU/RAM/disk usage, gateway PID, and context window fill level

### Version Alerts
S.A.M polls `npm view openclaw version` every 30 seconds to get the latest release. Any online agent running an older version triggers a **⚠ warning alert** in the TUI:

```
dellr720 outdated: 2026.2.19 → 2026.3.13 (Shift+U to update)
```

Press **Shift+U** with an agent selected to trigger an in-TUI update flow that SSHes in, runs `npm update -g openclaw`, and restarts the gateway service.

### Detection Flow Summary

```
refresh() every 30s
  │
  ├─ tailscale status --json  ← single fast call
  │    │
  │    ├─ node OFFLINE → mark Offline immediately (no SSH)
  │    │
  │    └─ node ONLINE → SSH probe
  │         │
  │         ├─ fast (4/5): echo ok → Online + latency
  │         │
  │         └─ full (1/5): metrics, version, gateway PID
  │
  └─ npm view openclaw version → version alert if any agent is behind
```

## Architecture

```mermaid
graph TD
    subgraph Master["Master Node"]
        TUI["sam TUI / CLI"]
        DB[(MySQL Database)]
    end

    subgraph Fleet["Agent Fleet (Tailscale mesh)"]
        A1["Agent 1\n(OpenClaw)"]
        A2["Agent 2\n(OpenClaw)"]
        AN["Agent N\n(OpenClaw)"]
    end

    TUI -- "SSH (health probe / chat fallback)" --> A1
    TUI -- "SSH (health probe / chat fallback)" --> A2
    TUI -- "SSH (health probe / chat fallback)" --> AN
    TUI -- "SQL (fleet state, chat, tasks)" --> DB
    A1 -- "HTTP /v1/chat/completions" --> TUI
    A2 -- "HTTP /v1/chat/completions" --> TUI
```

For a deeper dive, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Configuration Reference

### `~/.config/sam/config.toml`

| Key | Default | Description |
|-----|---------|-------------|
| `database.host` | `127.0.0.1` | MySQL host |
| `database.port` | `3306` | MySQL port |
| `database.user` | `root` | MySQL user |
| `database.password` | _(empty)_ | MySQL password |
| `database.database` | `sam_fleet` | Database name |
| `tui.theme` | `standard` | Color theme: `standard`, `noir`, `paper`, `1977`, `2077`, `matrix`, `sunset`, `arctic` |
| `tui.background` | `dark` | Background style: `dark` or `light` |
| `tui.refresh_interval` | `30` | Fleet refresh interval in seconds |
| `tui.chat_poll_interval` | `3` | Chat poll interval in seconds |
| `tui.vim_mode` | `false` | Enable Vim keybinding mode (`hjkl`, `gg`, `G`, `:q`, `:w`) |
| `identity.user` | `operator` | Operator display name in chat |

Environment variables are also supported via a `.env` file:

| Variable | Description |
|----------|-------------|
| `SAM_DB_URL` | Full MySQL URL (overrides all `database.*` keys) |
| `SAM_DB_HOST` | MySQL host |
| `SAM_DB_PORT` | MySQL port |
| `SAM_DB_USER` | MySQL user |
| `SAM_DB_PASS` | MySQL password |
| `SAM_DB_NAME` | Database name |
| `SAM_SELF_IP` | IP of the master node |
| `SAM_FLEET_CONFIG` | Path to `fleet.toml` |

### `fleet.toml`

| Key | Required | Description |
|-----|----------|-------------|
| `agent[].name` | ✅ | Agent identifier (lowercase, used in DB) |
| `agent[].display` | ❌ | Human-friendly display name |
| `agent[].emoji` | ❌ | Emoji prefix shown in TUI |
| `agent[].location` | ❌ | Physical/logical location label |
| `agent[].ssh_user` | ❌ | SSH username (default: `root`) |
| `agent[].jump_host` | ❌ | Optional SSH bastion/jump host (`-J`) |
| `agent[].jump_user` | ❌ | Optional jump-host SSH user (defaults to `agent[].ssh_user`) |

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, coding style, and the PR process.

Quick summary:
1. Fork the repo and create a feature branch
2. `cargo build && cargo test` must pass
3. Open a pull request against `main`

## Troubleshooting

**`sam` hangs on startup**
Check your MySQL connection. Run `sam doctor` — it will report which agents and DB connections are failing.

**Agent shows `offline` but is reachable by SSH**
The SSH health probe uses `BatchMode=yes` and a 5-second timeout. Ensure key-based auth is configured and the agent's SSH port is accessible over Tailscale.

**`sam init` fails with "permission denied"**
The installer tries to write to `/usr/local/bin`. Run with `sudo` or set `INSTALL_DIR` to a writable path.

**Chat messages stay `pending`**
The OpenClaw HTTP API on the agent may be unavailable. S.A.M will fall back to SSH delivery, but the agent must be online. Check `sam doctor --agent <name>`.

**Mermaid diagram not rendering**
GitHub renders Mermaid natively in markdown. If you see raw code, ensure you are viewing the file on github.com (not a local preview that lacks Mermaid support).

## License

MIT
