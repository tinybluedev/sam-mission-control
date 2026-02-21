# 🛰️ S.A.M Mission Control

**The next step in AI agent orchestration.**

We've been in the LLM space since GPT-2 — before chatbots went mainstream, before the hype cycle. We built our own inference clusters, ran local models when people still thought you needed a data center. We were there when iterative TUI coding agents emerged, and we hacked together our own orchestration layer with tmux injections from a gateway poller before tools like OpenClaw existed. OpenClaw was just the polished version of what we'd already been doing.

Now we're building what comes next: **Mission Control.**

Not another chatbot. Not another wrapper. A real-time fleet orchestration layer for coordinated AI agent deployment across distributed infrastructure — from bare metal to VPS to mobile devices.

<p align="center">
  <img src="docs/screenshot-dashboard.svg" alt="S.A.M Mission Control Dashboard" width="100%">
</p>

## The Vision

The evolution looks like this:

```
GPT → Chatbots → TUI Coding Agents → OpenClaw Agents → Mission Control
```

Each step gave us more agency. Mission Control is where it becomes **coordinated** — spawn agents across your fleet, pair them on problems, route tasks by capability, and watch it all happen from a single terminal.

## What It Does

- **Fleet Dashboard** — real-time status of every agent in your fleet, probed via SSH over your mesh VPN
- **Live Chat** — send commands to any agent with `@agent message`, see responses stream back
- **Agent Detail** — drill into any machine for full system info (OS, kernel, version, capabilities)
- **DB-Backed State** — fleet status persists in MySQL so agents can self-report via heartbeat
- **Auto-Refresh** — fleet probes every 30s, chat polls every 3s
- **Zero Network Exposure** — no web interface, no open ports. SSH and Unix socket only.

## Architecture

```
┌─────────────────────────────────────────────┐
│           S.A.M Mission Control             │
│         (Rust TUI on your hub node)         │
├─────────────────────────────────────────────┤
│                                             │
│   Fleet Panel          Chat Panel           │
│   ┌──────────┐    ┌──────────────────┐      │
│   │ Agent 1 ●│    │ @agent1 do thing │      │
│   │ Agent 2 ●│    │ ↳ done, result X │      │
│   │ Agent 3 ○│    │ @agent2 status   │      │
│   │ ...      │    │ ⏳ pending...    │      │
│   └──────────┘    └──────────────────┘      │
│                                             │
├─────────────┬───────────────────────────────┤
│   SSH Mesh  │        MySQL (State)          │
│  (Tailscale │   mc_fleet_status             │
│   WireGuard │   mc_chat                     │
│   etc.)     │   mc_task_routing             │
└─────────────┴───────────────────────────────┘
        │                    │
   ┌────┴────┐          ┌───┴───┐
   │ Agent 1 │          │ Agent │
   │ Agent 2 │  . . .   │  N    │
   │ Agent 3 │          │       │
   └─────────┘          └───────┘
```

Each agent is an AI-powered machine in your fleet. Mission Control talks to them over SSH (through your VPN mesh), tracks their state in a shared database, and routes commands through a chat-based interface.

## Quick Start

### Prerequisites

- **Rust** (1.70+)
- **MySQL/MariaDB** database
- **SSH access** to your fleet (key-based auth recommended)
- **VPN mesh** (Tailscale, WireGuard, Headscale, etc.) for secure cross-network connectivity

### Build

```bash
git clone https://github.com/tinybluedev/sam-mission-control.git
cd sam-mission-control
cargo build --release
```

### Configure

```bash
# 1. Database credentials
cp .env.example .env
# Edit .env with your MySQL connection details

# 2. Fleet definition
cp fleet.example.toml fleet.toml
# Edit fleet.toml — add your machines
```

**`.env`**
```env
SAM_DB_URL=mysql://user:password@dbhost:3306/your_database
SAM_SELF_IP=10.0.0.1        # This machine's mesh VPN IP
SAM_USER=operator            # Your display name in chat
```

**`fleet.toml`**
```toml
[[agent]]
name = "webserver"
display = "Web Server"
emoji = "🌐"
location = "Cloud"
ssh_user = "deploy"

[[agent]]
name = "gpu-node"
display = "GPU Node"
emoji = "🔥"
location = "Home"
ssh_user = "admin"
```

### Database Setup

Create the required tables in your MySQL database:

```sql
CREATE TABLE mc_fleet_status (
    agent_name VARCHAR(64) PRIMARY KEY,
    hostname VARCHAR(128),
    tailscale_ip VARCHAR(45),
    status ENUM('online','busy','offline','error') DEFAULT 'offline',
    current_task_id INT,
    last_heartbeat DATETIME,
    oc_version VARCHAR(32),
    os_info VARCHAR(128),
    kernel VARCHAR(64),
    capabilities JSON,
    token_burn_today INT DEFAULT 0,
    uptime_seconds BIGINT DEFAULT 0,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

CREATE TABLE mc_chat (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    sender VARCHAR(64) NOT NULL,
    target VARCHAR(64),
    message TEXT NOT NULL,
    response TEXT,
    status ENUM('pending','delivered','responded','failed') DEFAULT 'pending',
    created_at DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3),
    responded_at DATETIME(3),
    INDEX idx_target_status (target, status),
    INDEX idx_created (created_at)
);

-- Seed your fleet (one row per agent matching fleet.toml names)
INSERT INTO mc_fleet_status (agent_name) VALUES ('webserver'), ('gpu-node');
```

### Run

```bash
# From the project directory (loads .env and fleet.toml)
./target/release/sam-mission-control

# Or install globally
sudo cp target/release/sam-mission-control /usr/local/bin/sam
sam
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Tab` | Switch focus (Fleet ↔ Chat) |
| `↑↓` / `jk` | Navigate fleet list |
| `Enter` | Open agent detail view |
| `r` | Refresh all agents (SSH probe) |
| `@name msg` | Send command to agent |
| `PgUp/PgDn` | Scroll chat history |
| `?` | Help |
| `q` | Quit |

Agent names resolve by exact match, display name, or prefix — so `@web` matches `webserver`, `@gpu` matches `gpu-node`.

## Stack

| Component | Tech |
|-----------|------|
| TUI | [Ratatui](https://ratatui.rs) |
| Async | [Tokio](https://tokio.rs) |
| Database | [mysql_async](https://docs.rs/mysql_async) |
| Config | [TOML](https://toml.io) + [dotenvy](https://docs.rs/dotenvy) |
| Agent comms | SSH (via your existing mesh) |
| Language | Rust |

## Security Model

- **No web interface.** No HTTP server, no WebSocket, no open ports.
- **No hardcoded credentials.** All secrets live in `.env` (gitignored).
- **No fleet data in source.** Agent definitions live in `fleet.toml` (gitignored).
- **SSH-only communication.** Rides your existing VPN mesh. No new attack surface.
- **Single binary.** ~6MB, zero runtime dependencies.

## Roadmap

- [x] Fleet dashboard with SSH probing
- [x] Live chat with agent targeting
- [x] Agent detail view
- [x] DB-backed persistent state
- [x] Externalized config (zero secrets in source)
- [ ] **Pair Rooms** — put two agents in a sandbox to collaborate on a problem
- [ ] **Task Routing** — auto-assign work based on agent capabilities (GPU, network, OS)
- [ ] **Agent Heartbeat** — agents poll `mc_chat` and respond autonomously
- [ ] **Fleet Deploy** — push updates across the fleet from the TUI
- [ ] **Session Streaming** — watch agent work in real-time within the TUI
- [ ] **Plugin System** — extend with custom probes, commands, and integrations

## Philosophy

The best orchestration tool is the one with no attack surface. Web dashboards are liabilities. Mission Control is a terminal application that runs on your hub node, talks to your fleet over SSH, and stores state in a database only your machines can reach.

If someone compromises your TUI, they already have SSH access to your hub — at which point a web dashboard wouldn't have saved you anyway. Reduce the surface. Keep it simple. Keep it fast.

## Contributing

PRs welcome. The codebase is intentionally small — `main.rs`, `db.rs`, `config.rs`. Read the code, understand the architecture, submit focused changes.

## License

MIT
