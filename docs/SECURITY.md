# S.A.M Mission Control — Security Model

## Design Philosophy

S.A.M is designed with a **zero-exposure** principle: the control plane never opens listening ports. All communication is initiated from the master node outbound. There is no web API, no REST endpoint, and no agent that accepts inbound connections from S.A.M.

---

## What Is Exposed

| Surface | Exposure | Notes |
|---------|----------|-------|
| MySQL database | Local network only | Bind to Tailscale IP or loopback; never expose to internet |
| SSH | Tailscale mesh (encrypted) | Standard OpenSSH, key-based auth only |
| OpenClaw HTTP gateway | Tailscale mesh (encrypted) | Agent gateway listens on loopback, tunnelled via SSH |
| S.A.M binary | None | No listening socket |

---

## Threat Model

### In Scope

- **Credential leakage**: passwords in config files or environment variables should be protected with filesystem permissions (`chmod 600 ~/.config/sam/config.toml`).
- **SSH key compromise**: if an SSH key is stolen, an attacker gains access to all agents that key can reach. Use per-agent keys where possible.
- **Database access**: the MySQL database contains agent status, chat history, and task results. Restrict DB access to the master node's IP only.
- **Tailscale network access**: any device on your Tailscale network can reach agent gateway ports. Use Tailscale ACLs to restrict access to the master node only.

### Out of Scope

- **Physical access** to the master node or agent hardware.
- **Tailscale infrastructure compromise** — S.A.M assumes the mesh network is trusted.
- **Agent-side malicious code** — S.A.M trusts the output of SSH commands run on agents.

---

## Secrets Handling

### Config Files

- `~/.config/sam/config.toml` stores the database password in plain text.
- Recommended permissions: `chmod 600 ~/.config/sam/config.toml`
- Alternative: use environment variables (`SAM_DB_PASS`) sourced from a secrets manager.

### MySQL URLs

- When building connection URLs, special characters (`$`, `@`, `#`) in passwords are percent-encoded by `db::build_db_url()`.
- Error messages are sanitized by `db::sanitize_error()` to strip passwords before logging.

### Environment Variables

- A `.env` file in the working directory is loaded at startup via `dotenvy`.
- Never commit `.env` files to version control. The repository `.gitignore` excludes `.env`.

---

## SSH Security

- All SSH commands use `BatchMode=yes` — this prevents interactive prompts and ensures failures are reported rather than silently hanging.
- `StrictHostKeyChecking=no` is used for automation. This trades TOFU security for operability in dynamic fleets. In high-security environments, maintain a known-hosts file and remove this option.
- SSH keys should be unencrypted only on the master node, with filesystem permissions `600`.

---

## CI / Secret Scanning

The CI pipeline includes a `secret-scan` job that checks for:
- Hardcoded private IP addresses in source code
- Password patterns in source code

See `.github/workflows/ci.yml` for the scan rules.

---

## Reporting Vulnerabilities

Please report security vulnerabilities privately by opening a GitHub Security Advisory rather than a public issue. Do not include exploit details in public issues.
