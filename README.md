# 🛰️ S.A.M Mission Control

**Real-time fleet orchestration for coordinated AI agent deployment — from a single terminal.**

[![CI](https://github.com/tinybluedev/sam-mission-control/actions/workflows/ci.yml/badge.svg)](https://github.com/tinybluedev/sam-mission-control/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

<p align="center">
  <img src="docs/screenshot-dashboard.svg" alt="S.A.M Mission Control Dashboard" width="100%">
</p>

### Dashboard View

```
╭───────────────────────────────────────────────────────────────────────────╮
│  🛰️  S.A.M MISSION CONTROL   18/20 agents   ● live   ⟳ refreshing  ▌Fleet▐ │
╰───────────────────────────────────────────────────────────────────────────╯
╭─ Fleet ──────────────────────────────╮╭─ Chat ────────────────────────────╮
│     Agent         Location  Status   ││  16:22:14 nick →all               │
│  🖥️  agent-01      Home   ●  online  ││     system health check           │
│  🐘  agent-02      Cloud  ●  online  ││     ↳ All 20 agents reporting in  │
│  🔥  agent-03      Home   ◉  busy    ││                                   │
│  🌐  agent-04      VPS    ○  offline ││  16:24:01 nick →all               │
│  📱  agent-05      Mobile ●  online  ││     check disk space              │
│  ⚡  agent-06      SM     ●  online  ││     🔄 processing...              │
│  🐝  agent-07      Home   ●  online  ││                                   │
│  🗡️  agent-08      Home   ●  online  ││  16:25:33 nick →all               │
│  🎬  agent-09      Home   ●  online  ││     uptime report                 │
│  🐜  agent-10      Home   ?  unknown ││     ⏳ pending...                 │
│  ...               ...    ...        ││                                   │
╰──────────────────────────────────────╯╰───────────────────────────────────╯
                                        ╭─ broadcast to all ⏎ ─────────────╮
                                        │ › check memory usage▌             │
                                        ╰───────────────────────────────────╯
╭───────────────────────────────────────────────────────────────────────────╮
│  v0.9 │ 18/20 online │ theme:standard/dark │ r=refresh b=bg c=color ?=help │
╰───────────────────────────────────────────────────────────────────────────╯
```

### Agent Detail View

Select any agent and press Enter to open their dedicated view with private chat:

```
╭───────────────────────────────────────────────────────────────────────────╮
│  🐘 agent-02  —  ●  online    ▌Chat▐                                      │
╰───────────────────────────────────────────────────────────────────────────╯
╭─ Info ───────────────────────╮╭─ 🐘 agent-02 Chat ───────────────────────╮
│                              ││  16:30:01 nick →@agent-02                 │
│  Host           10.0.0.2     ││     check disk space                      │
│  Location       Cloud        ││     ↳ Disk: 142G/500G (28% used)         │
│  Status         ●  online    ││                                           │
│  OS             Ubuntu 24.04 ││  16:31:15 nick →@agent-02                 │
│  Kernel         6.18.7       ││     show running services                 │
│  OC Version     2026.2.21-2  ││     💭 thinking...                        │
│  SSH User       myuser       ││                                           │
│  Capabilities   gpu, docker  ││  16:32:44 nick →@agent-02                 │
│  Tokens Today   1,247        ││     tail the gateway logs                 │
│  Last Seen      16:35:27     ││     ⏳ pending...                         │
│  Task           none         ││                                           │
│                              ││                                           │
╰──────────────────────────────╯╰───────────────────────────────────────────╯
                                ╭─ @agent-02 › ────────────────────────────╮
                                │ › restart the gateway▌                    │
                                ╰───────────────────────────────────────────╯
╭───────────────────────────────────────────────────────────────────────────╮
│  Esc=back │ Tab=switch │ r=refresh agent-02 │ q=quit                       │
╰───────────────────────────────────────────────────────────────────────────╯
```

### Theme Gallery

Cycle themes with `c`, backgrounds with `b`. All 8 themes adapt to light and dark modes:

```
  standard ──── Clean cyan/blue (default)
  noir ──────── White/grey monochrome
  paper ─────── Black on white (light mode)
  1977 ──────── Warm amber/orange CRT feel
  2077 ──────── Neon cyberpunk pink/cyan
  matrix ────── Green phosphor terminal
  sunset ────── Warm orange/red/purple
  arctic ────── Cool blue/white/silver
```

## Features

- **20-agent fleet monitoring** — SSH-probed status for up to 20 agents in your fleet
- **Separated chat streams** — global broadcasts (dashboard) + private direct chat (agent detail)
- **Agent detail view** — dedicated chat and full system info per agent (OS, kernel, version, capabilities)
- **8 color themes + 5 background densities** — fully themeable TUI; cycle with `c` / `b`
- **MySQL-backed persistent state** — fleet status and chat history survive restarts
- **Non-blocking concurrent SSH probing** — all agents probed in parallel via Tokio tasks
- **Zero network exposure** — no web interface, no open ports; SSH and database only

## Architecture

```
  ┌──────────────────────────┐
  │   sam (Rust TUI)         │
  │   your hub node          │
  └────────┬─────────────────┘
           │ SQL reads/writes
           ▼
  ┌──────────────────────────┐
  │        MySQL             │
  │  mc_fleet_status         │
  │  mc_chat                 │
  └────────┬─────────────────┘
           │ polls mc_chat for tasks
           ▼
  ┌──────────────────────────┐
  │   mc_chat_responder      │
  │   (agent-side daemon)    │
  └────────┬─────────────────┘
           │ SSH
           ▼
  ┌──────────────────────────┐
  │   agents (fleet nodes)   │
  │  agent-01 … agent-20     │
  └──────────────────────────┘
```

Mission Control writes messages to `mc_chat`. Each agent runs `mc_chat_responder`, which polls the table, executes tasks locally, and writes responses back. The TUI polls for updates every 3 seconds. SSH is used for live status probing only.

## Setup

### Prerequisites

- **Rust 1.85+** — <https://rustup.rs>
- **MySQL or MariaDB** — database server accessible from your hub node
- **SSH key access** to every fleet node (key-based auth, no passwords)

### Build & Install

```bash
git clone https://github.com/tinybluedev/sam-mission-control.git
cd sam-mission-control
cargo build --release

# Install globally as `sam`
sudo cp target/release/sam-mission-control /usr/local/bin/sam
```

### Configuration

**`.env`** (copy from `.env.example`)
```env
# Option 1 — single URL
SAM_DB_URL=mysql://myuser:mypassword@10.0.0.1:3306/mission_control

# Option 2 — individual fields
SAM_DB_HOST=10.0.0.1
SAM_DB_PORT=3306
SAM_DB_USER=myuser
SAM_DB_PASS=mypassword
SAM_DB_NAME=mission_control

SAM_SELF_IP=10.0.0.1   # this hub node's IP (skips SSH probe for self)
SAM_USER=myuser         # your display name in chat
```

**`fleet.toml`** (copy from `fleet.example.toml`)
```toml
[[agent]]
name = "agent-01"
display = "Agent 01"
emoji = "🖥️"
location = "Home"
ssh_user = "myuser"

[[agent]]
name = "agent-02"
display = "Agent 02"
emoji = "🐘"
location = "Cloud"
ssh_user = "myuser"
```

Config files are loaded from (in order):
1. `$SAM_FLEET_CONFIG` / `./fleet.toml` / `~/.config/sam/fleet.toml`
2. `./.env` / `~/.config/sam/.env`

### Database Setup

```sql
CREATE TABLE mc_fleet_status (
    agent_name       VARCHAR(64) PRIMARY KEY,
    hostname         VARCHAR(128),
    tailscale_ip     VARCHAR(45),
    status           ENUM('online','busy','offline','error') DEFAULT 'offline',
    current_task_id  INT,
    last_heartbeat   DATETIME,
    oc_version       VARCHAR(32),
    os_info          VARCHAR(128),
    kernel           VARCHAR(64),
    capabilities     JSON,
    token_burn_today INT DEFAULT 0,
    uptime_seconds   BIGINT DEFAULT 0,
    updated_at       DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

CREATE TABLE mc_chat (
    id           BIGINT AUTO_INCREMENT PRIMARY KEY,
    sender       VARCHAR(64) NOT NULL,
    target       VARCHAR(64),
    message      TEXT NOT NULL,
    response     TEXT,
    status       ENUM('pending','delivered','responded','failed') DEFAULT 'pending',
    created_at   DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3),
    responded_at DATETIME(3),
    INDEX idx_target_status (target, status),
    INDEX idx_created (created_at)
);

-- Seed one row per agent matching the names in fleet.toml
INSERT INTO mc_fleet_status (agent_name) VALUES ('agent-01'), ('agent-02');
```

### Run

```bash
# From the project directory (picks up .env and fleet.toml automatically)
sam

# Or run directly without installing
./target/release/sam-mission-control
```

## Keybindings

| Key | Context | Action |
|-----|---------|--------|
| `Tab` | Dashboard | Switch focus: Fleet ↔ Chat |
| `↑` / `k` | Fleet focused | Move selection up |
| `↓` / `j` | Fleet focused | Move selection down |
| `Enter` | Fleet focused | Open agent detail view |
| `r` | Fleet focused | Refresh all agents (SSH probe) |
| `c` | Dashboard | Cycle color theme |
| `b` | Dashboard | Cycle background density |
| `?` | Dashboard | Open help screen |
| `q` | Dashboard | Quit |
| `Enter` | Agent Detail | Tab into agent's private chat |
| `msg` | Chat focused | Broadcast message to all agents |
| `msg` | Agent Chat | Send direct message to focused agent |
| `Enter` | Chat focused | Send message |
| `PgUp` / `PgDn` | Chat focused | Scroll chat history |
| `Esc` | Chat / Detail | Back / unfocus |

Agent names resolve by exact match, display name, or prefix — `@ag` matches `agent-01`.

## Themes

Cycle with `t`. Each theme adapts automatically to the current background density.

| Key | Theme | Description |
|-----|-------|-------------|
| 1 | `standard` | Cyan/blue — the default |
| 2 | `noir` | White/grey on black |
| 3 | `paper` | Black on white (light mode) |
| 4 | `1977` | Warm amber/orange/brown |
| 5 | `2077` | Neon pink/cyan/yellow |
| 6 | `matrix` | Green on black |
| 7 | `sunset` | Warm orange/red/purple |
| 8 | `arctic` | Cool blue/white/silver |

Background densities (cycle with `b`): `dark` · `medium` · `light` · `white` · `terminal`

## Stack

| Component | Tech |
|-----------|------|
| TUI | [Ratatui](https://ratatui.rs) |
| Async runtime | [Tokio](https://tokio.rs) |
| Database | [mysql_async](https://docs.rs/mysql_async) |
| Config | [TOML](https://toml.io) + [dotenvy](https://docs.rs/dotenvy) |
| Agent comms | SSH (via your existing VPN mesh) |
| Language | Rust 1.85+ |

## Security Model

- **No web interface.** No HTTP server, no WebSocket, no open ports.
- **No hardcoded credentials.** All secrets live in `.env` (gitignored).
- **No fleet data in source.** Agent definitions live in `fleet.toml` (gitignored).
- **SSH-only probing.** Rides your existing VPN mesh. No new attack surface.
- **Single binary.** ~6 MB, zero runtime dependencies.

## Contributing

PRs welcome. The codebase is intentionally small — `main.rs`, `db.rs`, `config.rs`, `theme.rs`. Read the code, understand the architecture, submit focused changes.

## License

MIT
