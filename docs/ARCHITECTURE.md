# S.A.M Mission Control — Architecture

## Overview

S.A.M is a single-binary Rust application that runs on a **master node** and manages a fleet of remote AI agents. It combines a full-screen TUI (built with [Ratatui](https://ratatui.rs)) with a CLI for scripting and automation.

All communication with agents is tunnelled through **SSH** or the **Tailscale mesh network**. There are no listening sockets on any agent — the control plane is entirely initiated from the master node.

---

## Component Overview

| Component | File(s) | Responsibility |
|-----------|---------|----------------|
| TUI / Main loop | `src/main.rs` | Event loop, rendering, keybindings |
| CLI dispatcher | `src/cli.rs` | Subcommand parsing (`init`, `status`, `doctor`, `onboard`, …) |
| Database layer | `src/db.rs` | MySQL queries, fleet state, chat, tasks |
| Config loader | `src/config.rs` | `config.toml` + `fleet.toml` parsing, alias resolution |
| Theme engine | `src/theme.rs` | 8 color themes, palette definitions |
| Setup wizard | `src/wizard.rs` | Interactive first-run configuration |
| Install script | `install.sh` | Curl-pipe installer, OS detection, Rust setup |

---

## Data Flow

### Health Probe (fleet refresh)

```
sam (master)
  │
  ├─ SSH: ssh -o BatchMode=yes -o ConnectTimeout=5 <agent>
  │       "uname -r && uptime && ..."
  │
  └─ Result → db::update_agent_status_full()
              → MySQL mc_fleet_status
```

### Chat (direct message)

```
operator types message in TUI
  │
  ├─ db::send_direct() → INSERT INTO mc_chat (status='pending')
  │
  ├─ HTTP POST http://<agent>:<gateway_port>/v1/chat/completions
  │     (OpenClaw HTTP gateway over Tailscale)
  │     on success → db::respond_to_chat() (status='responded')
  │
  └─ SSH fallback (if HTTP fails)
        ssh <agent> "curl -s localhost:<port>/v1/chat/completions ..."
```

### Task Routing

```
operator creates task
  │
  └─ db::create_task() → INSERT INTO mc_task_routing
        assigned_agent = <name>   → status = 'assigned'
        assigned_agent = NULL     → status = 'queued'

agent daemon (external)
  └─ polls mc_task_routing WHERE status='queued'|'assigned'
     executes task, updates status='completed'|'failed'
```

---

## Database Schema (key tables)

| Table | Purpose |
|-------|---------|
| `mc_fleet_status` | One row per agent: IP, status, versions, uptime, gateway port |
| `mc_chat` | All chat messages — direct and global broadcasts |
| `mc_task_routing` | Task queue with priority, assignment, and result |
| `mc_agent_crons` | Per-agent cron job definitions collected from agents |
| `mc_agent_context` | Token usage snapshots for context window tracking |
| `mc_spawned_agents` | Sub-agent spawn requests and results |

The schema is initialized by `sam init` which runs `CREATE TABLE IF NOT EXISTS` statements. No migration framework is used; tables are additive.

---

## Configuration Loading Order

1. `SAM_DB_URL` environment variable (full MySQL URL, highest priority)
2. `SAM_DB_HOST` / `SAM_DB_PORT` / `SAM_DB_USER` / `SAM_DB_PASS` / `SAM_DB_NAME` env vars
3. `~/.config/sam/config.toml` (parsed at startup)
4. Built-in defaults (`127.0.0.1:3306`, user `root`)

Fleet configuration (`fleet.toml`) is resolved in order:
1. `$SAM_FLEET_CONFIG` env var
2. `./fleet.toml` (current working directory)
3. `~/.config/sam/fleet.toml`

---

## SSH Command Patterns

All SSH commands use these standard options to ensure non-interactive, scripted operation:

```
-o ConnectTimeout=5
-o StrictHostKeyChecking=no
-o BatchMode=yes
```

On macOS agents, Homebrew-installed binaries require a PATH prefix:
```
export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; <command>
```

---

## Async Architecture

The main TUI loop is synchronous (driven by `crossterm` events). Background operations — SSH probes, HTTP chat requests, DB queries — are offloaded with `tokio::spawn` to avoid blocking the rendering thread.

```
tokio::main (async runtime)
  │
  ├─ TUI event loop (sync, crossterm)
  │
  └─ Background tasks (tokio::spawn)
        ├─ SSH health probes (per agent, concurrent)
        ├─ HTTP chat completions (60s timeout)
        └─ DB poll for pending messages
```
