# 🛰️ S.A.M Mission Control

Fleet orchestration TUI for AI agents. Monitor, chat with, and manage your entire fleet from one terminal.

## Quick Start

```bash
# Build
git clone https://github.com/tinybluedev/sam-mission-control.git
cd sam-mission-control
cargo build --release
sudo cp target/release/sam-mission-control /usr/local/bin/sam

# Init (creates DB tables, config, everything)
sam init --db-host 10.0.0.1 --db-pass 'yourpassword'

# Launch
sam
```

That's it. Three commands.

## Add Agents

```bash
# Onboard a machine (installs OpenClaw, configures gateway, registers in fleet)
sam onboard 10.64.0.3

# Push workspace files to agents
sam deploy all --file SOUL.md
sam deploy dellr720 --file AGENTS.md
```

## What It Does

| Screen | Key | Description |
|--------|-----|-------------|
| **Dashboard** | — | Fleet status, global chat, resource bars |
| **Agent Detail** | `Enter` | Private AI chat, config viewer, gateway logs |
| **Task Board** | `t` | Create and track fleet tasks |
| **Alerts** | `w` | Threshold-based notifications |
| **VPN Status** | `v` | Tailscale mesh overview |
| **Help** | `?` | Full keybinding reference |

## Keybindings

| Key | Action |
|-----|--------|
| `↑↓` / `jk` | Navigate |
| `Enter` | Agent detail |
| `Tab` | Switch panels |
| `Space` | Toggle select |
| `f` | Filter/search |
| `s` | Sort fleet |
| `/` | Run command on fleet |
| `g` | Restart gateway |
| `G` | Investigate gateway |
| `e` | View agent config |
| `a` | Add agent wizard |
| `r` | Refresh |
| `c` / `b` | Cycle theme / background |
| `?` | Help |
| `q` | Quit |

## CLI

```bash
sam                  # TUI
sam status           # Fleet status (non-interactive)
sam chat cyber "hi"  # Message an agent
sam onboard <ip>     # Provision new agent
sam deploy <target> --file <file>  # Push workspace files
sam init             # First-time setup
sam setup            # Config wizard
```

## Chat

Agent detail chat talks to **real AI agents** via OpenClaw's HTTP API. Each agent responds with its own personality, context, and tools.

Dashboard chat broadcasts to all agents simultaneously.

## Architecture

```
sam (Rust TUI) ──SQL──► MySQL (mc_fleet_status, mc_chat)
       │
       ├──SSH──► Agent probes (status, resources, latency)
       └──HTTP──► OpenClaw gateway /v1/chat/completions (AI chat)
```

- **Zero network exposure** — no HTTP server, no open ports on the hub
- **SSH + Tailscale mesh** — rides your existing VPN
- **Single binary** — ~6MB, zero runtime deps

## Requirements

- Rust 1.85+
- MySQL/MariaDB
- SSH key access to fleet nodes
- [OpenClaw](https://openclaw.ai) on each agent (installed automatically by `sam onboard`)

## License

MIT
