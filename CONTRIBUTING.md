# Contributing to S.A.M Mission Control

Thank you for your interest in contributing! This document covers how to build, test, and submit changes.

---

## Prerequisites

- **Rust 1.85+** — install via [rustup](https://rustup.rs):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **MySQL / MariaDB** — required for integration testing and running locally
- **SSH access** — key-based auth to any agents you want to test against

---

## Building

```bash
git clone https://github.com/tinybluedev/sam-mission-control.git
cd sam-mission-control
cargo build
```

For a release (optimized) build:

```bash
cargo build --release
# Binary: target/release/sam-mission-control
```

---

## Running Tests

```bash
cargo test
```

The test suite covers:
- `db::build_db_url` — URL construction and password encoding
- `db::sanitize_error` — credential masking in error messages
- `config::resolve_alias` — agent name resolution logic

No live MySQL connection is needed for the unit tests.

---

## Code Style

- Follow standard Rust idioms (`rustfmt` formatting, `clippy` lints)
- All public functions must have `///` doc comments
- All source modules must have a `//!` module-level doc comment
- Use `r.get::<Option<T>, _>(index).flatten()` for nullable DB columns (never unwrap directly)
- Do not hardcode IP addresses or passwords in source files — the CI `secret-scan` job will catch these

---

## Project Layout

```
src/
  main.rs       — TUI event loop and rendering
  cli.rs        — CLI subcommands (init, status, doctor, onboard, …)
  db.rs         — MySQL queries and data structs
  config.rs     — config.toml / fleet.toml parsing and alias resolution
  theme.rs      — color theme definitions
  wizard.rs     — interactive first-run setup wizard

docs/
  ARCHITECTURE.md   — system design and data flow
  SECURITY.md       — security model and threat model
  CHANGELOG.md      — version history

install.sh          — curl-pipe installer
config.example.toml — annotated config template
fleet.example.toml  — annotated fleet config template
```

---

## Submitting a Pull Request

1. Fork the repository and create a feature branch from `main`:
   ```bash
   git checkout -b feat/my-feature
   ```

2. Make your changes. Ensure `cargo build` and `cargo test` pass:
   ```bash
   cargo build && cargo test
   ```

3. Commit with a clear message following [Conventional Commits](https://www.conventionalcommits.org/) style:
   ```
   feat: add agent health score to dashboard
   fix: prevent NULL panic in load_fleet when kernel is NULL
   docs: add module-level comments to db.rs
   ```

4. Push and open a pull request against `main`. Fill in the PR template describing what changed and why.

5. CI must pass (`build-and-test`, `security-audit`, `secret-scan`) before merge.

---

## Reporting Issues

- **Bugs**: open a GitHub Issue with steps to reproduce, expected vs. actual behaviour, and `sam version` output.
- **Security vulnerabilities**: use a GitHub Security Advisory (private). Do not open a public issue.
- **Feature requests**: open a GitHub Issue with a clear use-case description.
