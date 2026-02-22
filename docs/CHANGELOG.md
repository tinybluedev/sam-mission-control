# Changelog

All notable changes to S.A.M Mission Control are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## [1.2.0] — 2026-02

### Added
- **Task Engine** — `mc_task_routing` DB table, cron collector, spawn manager scaffold
- **Agent Cron Collection** — `mc_agent_crons` table; cron jobs are collected from agents and displayed in detail view
- **Context Snapshots** — `mc_agent_context` table tracks per-agent token usage and context window percentage
- **Spawned Agents** — `mc_spawned_agents` table for sub-agent spawn requests and results
- **Fleet Doctor** — `sam doctor` and `sam doctor --fix` commands to diagnose and auto-repair fleet issues
- **SSH Fallback Chat** — when OpenClaw HTTP gateway is unreachable, chat messages are delivered over SSH
- **macOS Agent Support** — PATH prefix for Homebrew-installed tools on macOS agents

### Changed
- SSH commands now consistently use `-o ConnectTimeout=5 -o StrictHostKeyChecking=no -o BatchMode=yes`
- Agent names are stored in lowercase in the database

### Fixed
- NULL panic in `db::load_fleet` for optional columns — all `Row::get` calls use `Option<T>` with `.flatten()`
- Password special characters (`$`, `@`, `#`) are percent-encoded in MySQL URLs

---

## [1.1.0] — 2026-01

### Added
- **Multi-select** — batch operations on groups of agents
- **8 Color Themes** — Standard, Noir, Paper, 1977, 2077, Matrix, Sunset, Arctic
- **Task Board** — create, assign, and track tasks across agents (`mc_task_routing`)
- **Agent Detail** — deep-dive view with direct messaging and cron/context panels
- **Global Broadcast** — send a message to all agents simultaneously

### Changed
- Theme colors are now defined in a central `Theme` struct; all UI rendering uses theme fields

---

## [1.0.0] — 2025-12

### Added
- Initial release
- **Fleet Dashboard** — real-time agent status with SSH health probes
- **AI Chat** — talk to agents via OpenClaw HTTP API
- `sam init` — interactive first-time setup (DB tables, config, fleet.toml)
- `sam onboard <ip>` — provision a new agent over SSH
- `sam deploy <agent> --file <path>` — push files to agent workspace
- `sam status` — non-interactive fleet status output
- `sam version` — print version
- MySQL-backed fleet state (`mc_fleet_status`, `mc_chat`)
- `config.toml` and `fleet.toml` configuration files
- Password sanitization in error messages
