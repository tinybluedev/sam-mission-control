# 🛰️ S.A.M Mission Control

Fleet orchestration TUI for AI agents.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/tinybluedev/sam-mission-control/main/install.sh | bash
```

Or build manually:

```bash
git clone https://github.com/tinybluedev/sam-mission-control.git
cd sam-mission-control
cargo build --release
sudo cp target/release/sam-mission-control /usr/local/bin/sam
```

## Setup

```bash
sam init
```

That's it. The wizard walks you through everything — database connection, config, and self-registration. All interactive. Pass flags to skip prompts:

```bash
sam init --db-host 10.0.0.1 --db-pass 'secret'
```

## Usage

```bash
sam                           # Launch TUI
sam onboard <ip>              # Add an agent to your fleet
sam deploy all --file SOUL.md # Push files to agents
sam status                    # Quick fleet check
sam chat cyber "hello"        # Message an agent
```

## What You Get

- **Fleet dashboard** — live status of every agent with SSH probing
- **AI chat** — talk to agents through OpenClaw's API (real AI responses)
- **Resource monitoring** — CPU, RAM, disk with color-coded bars
- **Alerts** — automatic notifications when things go wrong
- **Task board** — create and track fleet work
- **One-command onboarding** — `sam onboard <ip>` handles everything
- **8 themes** — cycle with `c`, backgrounds with `b`

## Keys

`?` in the TUI shows all keybindings. The important ones:

| Key | What it does |
|-----|-------------|
| `Enter` | Open agent detail + chat |
| `f` | Filter/search agents |
| `/` | Run command across fleet |
| `g` | Restart agent gateway |
| `e` | View agent config |
| `t` | Task board |
| `w` | Alerts |

## Requirements

- MySQL or MariaDB (any version)
- SSH key access to your machines
- [OpenClaw](https://openclaw.ai) on each agent (auto-installed by `sam onboard`)

## License

MIT
