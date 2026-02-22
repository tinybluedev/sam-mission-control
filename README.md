# S.A.M Mission Control

A terminal-based fleet orchestration tool for managing distributed AI agents over SSH and Tailscale mesh networks. Built in Rust with [Ratatui](https://ratatui.rs).

![Dashboard](docs/screenshots/dashboard.png)

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
| `r` | Refresh all agents (SSH) |
| `a` | New agent wizard |
| `g` | Restart gateway (selected) |
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

## Architecture

```
┌─────────────┐     SSH      ┌──────────┐
│  sam (TUI)  │─────────────▶│ Agent 1  │
│  on master  │     SSH      ├──────────┤
│  node       │─────────────▶│ Agent 2  │
│             │     SSH      ├──────────┤
│             │─────────────▶│ Agent N  │
└──────┬──────┘              └──────────┘
       │
       ▼
┌─────────────┐
│   MySQL DB  │
│ (fleet state│
│  chat, tasks│
└─────────────┘
```

## License

MIT
