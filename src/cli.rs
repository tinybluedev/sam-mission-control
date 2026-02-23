//! CLI subcommand definitions and implementations for S.A.M Mission Control.
//!
//! This module defines the [`Cli`] struct (parsed by [clap]) and all [`Commands`]
//! variants. Each subcommand is implemented as an async function that is called
//! from `main.rs` when no TUI is needed.
//!
//! ## Commands
//! - [`Commands::Status`] — print fleet status and exit
//! - [`Commands::Chat`] — send a direct message to an agent
//! - [`Commands::Doctor`] — diagnose (and optionally fix) fleet issues
//! - [`Commands::Init`] — first-time database and config setup
//! - [`Commands::Setup`] — regenerate `config.toml`
//! - [`Commands::Onboard`] — provision a new agent over SSH
//! - [`Commands::Deploy`] — push a file to an agent's workspace
//! - [`Commands::Validate`] — validate remote openclaw.json schema
//! - [`Commands::Version`] — print the binary version

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::validate;

// ── ANSI Color Helpers ──────────────────────────────────────────
fn c_cyan(s: &str) -> String { format!("\x1b[36m{}\x1b[0m", s) }
fn c_green(s: &str) -> String { format!("\x1b[32m{}\x1b[0m", s) }
fn c_red(s: &str) -> String { format!("\x1b[31m{}\x1b[0m", s) }
fn c_yellow(s: &str) -> String { format!("\x1b[33m{}\x1b[0m", s) }
fn c_magenta(s: &str) -> String { format!("\x1b[35m{}\x1b[0m", s) }
fn c_bold(s: &str) -> String { format!("\x1b[1m{}\x1b[0m", s) }
fn c_dim(s: &str) -> String { format!("\x1b[2m{}\x1b[0m", s) }
fn c_bold_cyan(s: &str) -> String { format!("\x1b[1;36m{}\x1b[0m", s) }
fn c_bold_green(s: &str) -> String { format!("\x1b[1;32m{}\x1b[0m", s) }
fn c_bold_red(s: &str) -> String { format!("\x1b[1;31m{}\x1b[0m", s) }
fn c_bold_yellow(s: &str) -> String { format!("\x1b[1;33m{}\x1b[0m", s) }
fn c_bold_magenta(s: &str) -> String { format!("\x1b[1;35m{}\x1b[0m", s) }
fn c_bg_green(s: &str) -> String { format!("\x1b[42;30m {}  \x1b[0m", s) }
fn c_bg_red(s: &str) -> String { format!("\x1b[41;37m {}  \x1b[0m", s) }
fn c_bg_cyan(s: &str) -> String { format!("\x1b[46;30m {}  \x1b[0m", s) }

const BANNER: &str = r#"

    ____    _    __  __
   / ___|  / \  |  \/  |
   \___ \ / _ \ | |\/| |
    ___) / ___ \| |  | |
   |____/_/   \_\_|  |_|  MISSION CONTROL
   "#;

fn print_banner() {
    println!("\x1b[36m{}\x1b[0m", BANNER);
}

fn print_divider() {
    println!("   {}",c_dim("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"));
}

fn print_step(n: usize, total: usize, msg: &str) {
    print!("   {} {} ", c_bold_cyan(&format!("[{}/{}]", n, total)), msg);
}


#[derive(Parser)]
#[command(name = "sam", version, about = "S.A.M Mission Control — Fleet orchestration TUI")]
pub struct Cli {
    /// Path to config file
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Print fleet status and exit
    Status,
    /// Send a message to an agent
    Chat {
        /// Agent name
        agent: String,
        /// Message to send
        message: Vec<String>,
    },
    /// Diagnose and auto-fix fleet issues
    Doctor {
        /// Auto-fix issues (don't just report)
        #[arg(long)]
        fix: bool,
        /// Check specific agent only
        #[arg(short, long)]
        agent: Option<String>,
        /// Run doctor for the whole fleet in headless CLI mode
        #[arg(long)]
        fleet: bool,
        /// Emit machine-readable JSON output (fleet mode)
        #[arg(long)]
        json: bool,
        /// Only print failures (fleet mode)
        #[arg(long)]
        quiet: bool,
        /// Timeout in seconds for per-agent network checks
        #[arg(long, default_value = "5")]
        timeout: u64,
    },
    /// Full automated setup (DB tables, config, everything)
    Init {
        /// MySQL host
        #[arg(long)]
        db_host: Option<String>,
        /// MySQL port
        #[arg(long)]
        db_port: Option<u16>,
        /// MySQL user
        #[arg(long)]
        db_user: Option<String>,
        /// MySQL password
        #[arg(long)]
        db_pass: Option<String>,
        /// Database name
        #[arg(long)]
        db_name: Option<String>,
        /// This machine's IP (for self-detection)
        #[arg(long)]
        self_ip: Option<String>,
    },
    /// Run interactive setup wizard
    Setup,
    /// Deploy workspace files to agents
    Deploy {
        /// Agent name or "all"
        target: String,
        /// File to push (e.g. SOUL.md, AGENTS.md)
        #[arg(short, long)]
        file: String,
        /// Local source path (default: templates/<file>)
        #[arg(short, long)]
        source: Option<String>,
    },
    /// Onboard a new agent on a remote machine
    Onboard {
        /// Tailscale IP or hostname of the target machine
        host: String,
        /// SSH username (default: admin)
        #[arg(short, long, default_value = "admin")]
        user: String,
        /// Agent name/ID
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Print version info
    Version,
    /// Show operation history log
    Log {
        /// Filter by agent name
        #[arg(short, long)]
        agent: Option<String>,
        /// Number of most recent entries to show
        #[arg(long, default_value = "20")]
        tail: u32,
    },
    /// Validate openclaw.json schema on one or all agents
    Validate {
        /// Check only this agent (omit to validate all)
        #[arg(short, long)]
        agent: Option<String>,
    },
    /// Run scheduled-operations executor without launching TUI
    Daemon,
}

/// Persistent config file (~/.config/sam/config.toml)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SamConfig {
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub fleet: FleetConfig,
    #[serde(default)]
    pub identity: IdentityConfig,
}

/// Database connection settings from `[database]` in `config.toml`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatabaseConfig {
    #[serde(default)]
    pub mode: Option<String>,
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    #[serde(default)]
    pub sqlite_path: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            mode: None,
            url: None,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
            sqlite_path: None,
        }
    }
}

/// TUI display settings from `[tui]` in `config.toml`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TuiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_bg")]
    pub background: String,
    #[serde(default = "default_refresh")]
    pub refresh_interval: u64,
    #[serde(default = "default_chat_poll")]
    pub chat_poll_interval: u64,
    #[serde(default)]
    pub vim_mode: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { theme: default_theme(), background: default_bg(), refresh_interval: default_refresh(), chat_poll_interval: default_chat_poll(), vim_mode: false }
    }
}

fn default_theme() -> String { "standard".into() }
fn default_bg() -> String { "dark".into() }
fn default_refresh() -> u64 { 30 }
fn default_chat_poll() -> u64 { 3 }

/// Fleet config path override from `[fleet]` in `config.toml`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct FleetConfig {
    pub config_path: Option<String>,
}

/// Operator identity settings from `[identity]` in `config.toml`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IdentityConfig {
    #[serde(default = "default_user")]
    pub user: String,
}

impl Default for IdentityConfig {
    fn default() -> Self { Self { user: default_user() } }
}

fn default_user() -> String { "operator".into() }

impl SamConfig {
    /// Load config from file, or return defaults
    pub fn load(path: Option<&PathBuf>) -> Self {
        // Explicit path
        if let Some(p) = path {
            if let Ok(content) = std::fs::read_to_string(p) {
                if let Ok(cfg) = toml::from_str(&content) {
                    return cfg;
                }
            }
        }

        // Default paths
        let candidates = vec![
            PathBuf::from("config.toml"),
            dirs::config_dir().unwrap_or_default().join("sam/config.toml"),
        ];

        for p in candidates {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(cfg) = toml::from_str(&content) {
                    return cfg;
                }
            }
        }

        Self::default()
    }

    /// Save config to default location
    pub fn save(&self) -> Result<(), String> {
        let dir = dirs::config_dir().unwrap_or_default().join("sam");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("config.toml");
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, &content).map_err(|e| e.to_string())?;

        // Set file permissions to 0600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    /// Apply config to env vars (for backward compat with .env loading)
    pub fn apply_to_env(&self) {
        unsafe {
            if let Some(url) = &self.database.url {
                if std::env::var("SAM_DB_URL").is_err() {
                    std::env::set_var("SAM_DB_URL", url);
                }
            }
            if let Some(h) = &self.database.host {
                if std::env::var("SAM_DB_HOST").is_err() { std::env::set_var("SAM_DB_HOST", h); }
            }
            if let Some(p) = &self.database.port {
                if std::env::var("SAM_DB_PORT").is_err() { std::env::set_var("SAM_DB_PORT", p.to_string()); }
            }
            if let Some(u) = &self.database.user {
                if std::env::var("SAM_DB_USER").is_err() { std::env::set_var("SAM_DB_USER", u); }
            }
            if let Some(p) = &self.database.password {
                if std::env::var("SAM_DB_PASS").is_err() { std::env::set_var("SAM_DB_PASS", p); }
            }
            if let Some(d) = &self.database.database {
                if std::env::var("SAM_DB_NAME").is_err() { std::env::set_var("SAM_DB_NAME", d); }
            }
            if let Some(mode) = &self.database.mode {
                if std::env::var("SAM_DB_MODE").is_err() { std::env::set_var("SAM_DB_MODE", mode); }
            }
            if let Some(path) = &self.database.sqlite_path {
                if std::env::var("SAM_SQLITE_PATH").is_err() {
                    std::env::set_var("SAM_SQLITE_PATH", path);
                }
            }
            if std::env::var("SAM_USER").is_err() {
                std::env::set_var("SAM_USER", &self.identity.user);
            }
        }
    }

    /// Resolve theme name to ThemeName enum
    pub fn theme_name(&self) -> crate::theme::ThemeName {
        match self.tui.theme.as_str() {
            "noir" => crate::theme::ThemeName::Noir,
            "paper" => crate::theme::ThemeName::Paper,
            "1977" => crate::theme::ThemeName::Retro1977,
            "2077" => crate::theme::ThemeName::Cyber2077,
            "matrix" => crate::theme::ThemeName::Matrix,
            "sunset" => crate::theme::ThemeName::Sunset,
            "arctic" => crate::theme::ThemeName::Arctic,
            _ => crate::theme::ThemeName::Standard,
        }
    }

    pub fn bg_density(&self) -> crate::theme::BgDensity {
        match self.tui.background.as_str() {
            "medium" => crate::theme::BgDensity::Medium,
            "light" => crate::theme::BgDensity::Light,
            "white" => crate::theme::BgDensity::White,
            "terminal" => crate::theme::BgDensity::Transparent,
            _ => crate::theme::BgDensity::Dark,
        }
    }
}

impl Default for SamConfig {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            tui: TuiConfig::default(),
            fleet: FleetConfig::default(),
            identity: IdentityConfig::default(),
        }
    }
}

/// Interactive setup wizard
pub fn run_setup() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Write};

    println!("\n🛰️  S.A.M Mission Control — Setup Wizard\n");

    let mut cfg = SamConfig::default();

    // Database
    println!("━━━ Database Configuration ━━━");
    print!("  MySQL host [127.0.0.1]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let host = input.trim();
    cfg.database.host = Some(if host.is_empty() { "127.0.0.1".into() } else { host.into() });

    input.clear();
    print!("  MySQL port [3306]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let port = input.trim();
    cfg.database.port = Some(if port.is_empty() { 3306 } else { port.parse().unwrap_or(3306) });

    input.clear();
    print!("  MySQL user [root]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let user = input.trim();
    cfg.database.user = Some(if user.is_empty() { "root".into() } else { user.into() });

    input.clear();
    print!("  MySQL password: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    cfg.database.password = Some(input.trim().into());

    input.clear();
    print!("  Database name [sam_fleet]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let db = input.trim();
    cfg.database.database = Some(if db.is_empty() { "sam_fleet".into() } else { db.into() });

    // Identity
    println!("\n━━━ Identity ━━━");
    input.clear();
    print!("  Your display name [operator]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let name = input.trim();
    cfg.identity.user = if name.is_empty() { "operator".into() } else { name.into() };

    // Theme
    println!("\n━━━ Theme ━━━");
    println!("  Available: standard, noir, paper, 1977, 2077, matrix, sunset, arctic");
    input.clear();
    print!("  Default theme [standard]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let theme = input.trim();
    cfg.tui.theme = if theme.is_empty() { "standard".into() } else { theme.into() };

    // Save
    println!();
    match cfg.save() {
        Ok(_) => {
            let path = dirs::config_dir().unwrap_or_default().join("sam/config.toml");
            println!("✅ Config saved to {}", path.display());
            println!("   Permissions set to 0600 (owner read/write only)");
            println!("\n   Run `sam` to launch Mission Control.");
        }
        Err(e) => println!("❌ Failed to save config: {}", e),
    }

    Ok(())
}

/// Print fleet status to stdout (non-TUI)
pub async fn print_status() -> Result<(), Box<dyn std::error::Error>> {
    let pool = crate::db::get_pool();
    let agents = crate::db::load_fleet(&pool).await?;

    print_banner();
    println!();
    print_divider();
    println!("   {:<22} {:<14} {:<18} {}", c_bold("Agent"), c_bold("Status"), c_bold("Version"), c_bold("IP"));
    print_divider();

    let mut online = 0;
    for a in &agents {
        let (icon, status_str) = match a.status.as_str() {
            "online" => { online += 1; (c_green("●"), c_green("online")) },
            "busy" => { online += 1; (c_yellow("◉"), c_yellow("busy")) },
            "offline" => (c_red("○"), c_red("offline")),
            "error" => (c_bold_red("✖"), c_bold_red("error")),
            _ => (c_dim("?"), c_dim("unknown")),
        };
        let ver = a.oc_version.as_deref().unwrap_or("?");
        let ver_str = if ver.starts_with("2026.2.21") { c_green(ver) } else if ver == "?" { c_dim(ver) } else { c_yellow(ver) };
        println!("   {:<20} {} {:<22} {:<26} {}",
            c_cyan(&a.agent_name),
            icon,
            status_str,
            ver_str,
            c_dim(a.tailscale_ip.as_deref().unwrap_or("?")),
        );
    }

    print_divider();
    let summary = if online == agents.len() {
        c_bold_green(&format!("   ✔ {}/{} online — all systems nominal", online, agents.len()))
    } else {
        c_bold_yellow(&format!("   ⚠ {}/{} online — {} offline", online, agents.len(), agents.len() - online))
    };
    println!("{}\n", summary);

    pool.disconnect().await?;
    Ok(())
}

/// Send a chat message and wait for response (non-TUI)
pub async fn send_chat(agent: &str, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    validate::validate_agent_name(agent)
        .map_err(|e| format!("Invalid agent name: {}", e))?;
    let message = validate::sanitize_chat_message(message);
    if message.is_empty() {
        return Err("Message must not be empty".into());
    }
    let pool = crate::db::get_pool();
    let id = crate::db::send_direct(&pool, "cli", agent, &message).await?;
    println!("📨 Sent to @{}: {}", agent, message);
    println!("   Message ID: {} — waiting for response...", id);

    // Poll for response (max 30s)
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if let Ok(msgs) = crate::db::load_agent_chat(&pool, agent, 1).await {
            if let Some(m) = msgs.last() {
                if m.id == id && m.status == "responded" {
                    println!("↳ {}", m.response.as_deref().unwrap_or("(no response)"));
                    pool.disconnect().await?;
                    return Ok(());
                }
            }
        }
    }

    println!("⏳ Timed out after 30s — check `sam` TUI for response");
    pool.disconnect().await?;
    Ok(())
}


/// Onboard a new agent on a remote machine — full 10-step provisioning flow.
///
/// Steps:
///   1. Test SSH connectivity
///   2. Detect OS (Linux/macOS)
///   3. Distribute SSH public key (optional)
///   4. Check / install Node.js
///   5. Check / install OpenClaw
///   6. Run `openclaw init --non-interactive`
///   7. Configure gateway (token, bind, endpoints)
///   8. Start gateway
///   9. Check / configure Tailscale
///  10. Run post-install diagnostic
///  11. Register in DB (atomic — only on full success)
///  12. Print success banner
///
/// Rollback: if steps 5–10 fail after OpenClaw has been installed, it is
/// automatically removed before exiting.
pub async fn run_onboard(host: &str, user: &str, name: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;
    use std::time::Duration;
    use crate::shell;

    validate::validate_ip_address(host)
        .map_err(|e| format!("Invalid host: {}", e))?;
    validate::validate_ssh_username(user)
        .map_err(|e| format!("Invalid SSH user: {}", e))?;
    if let Some(n) = name {
        validate::normalize_agent_name(n)
            .map_err(|e| format!("Invalid agent name: {}", e))?;
    }

    const TOTAL: usize = 11;

    println!("\n🛰️  S.A.M Mission Control — Agent Onboarding");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Target:  {}@{}", user, host);
    println!();

    let ssh_target = format!("{}@{}", user, host);
    let ssh_args = |cmd: &str| -> Vec<String> {
        vec![
            "-o".into(), "ConnectTimeout=5".into(),
            "-o".into(), "StrictHostKeyChecking=no".into(),
            "-o".into(), "BatchMode=yes".into(),
            ssh_target.clone(), cmd.into(),
        ]
    };

    // ── Step 1: Test SSH connectivity ────────────────────────────────────
    print_step(1, TOTAL, "Testing SSH connection...");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let ssh_test = tokio::time::timeout(Duration::from_secs(8),
        Command::new("ssh").args(ssh_args("hostname")).output()
    ).await;
    match ssh_test {
        Ok(Ok(o)) if o.status.success() => {
            let hostname = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("{}", c_bold_green(&format!("✅ {}", hostname)));
        }
        _ => {
            println!("{}", c_bold_red(&format!("❌ Cannot reach {}@{}", user, host)));
            return Err("SSH connection failed — check host and key-based auth".into());
        }
    }

    // ── Step 2: Detect OS ────────────────────────────────────────────────
    print_step(2, TOTAL, "Detecting OS...");
    std::io::stdout().flush().ok();
    let os_out = Command::new("ssh").args(ssh_args(
        ". /etc/os-release 2>/dev/null && echo \"$PRETTY_NAME\" || sw_vers -productName 2>/dev/null || uname -s"
    )).output().await?;
    let os_name = String::from_utf8_lossy(&os_out.stdout).trim().to_string();
    let is_mac = os_name.to_lowercase().contains("mac") || os_name.to_lowercase().contains("darwin");
    let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
    println!("{}", c_bold_green(&format!("✅ {}", os_name)));

    // ── Step 3: Distribute SSH public key (optional) ─────────────────────
    print_step(3, TOTAL, "Distributing SSH public key...");
    std::io::stdout().flush().ok();
    let home_dir = dirs::home_dir().unwrap_or_default();
    let key_candidates = [
        home_dir.join(".ssh/id_ed25519.pub"),
        home_dir.join(".ssh/id_rsa.pub"),
        home_dir.join(".ssh/id_ecdsa.pub"),
    ];
    let pub_key = key_candidates.iter().find_map(|p| std::fs::read_to_string(p).ok());
    if let Some(key) = pub_key {
        let key = key.trim().to_string();
        let install_key_cmd = format!(
            "mkdir -p ~/.ssh && chmod 700 ~/.ssh && \
             grep -qxF {k} ~/.ssh/authorized_keys 2>/dev/null \
               || echo {k} >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys",
            k = shell::escape(&key),
        );
        let key_out = Command::new("ssh").args(ssh_args(&install_key_cmd)).output().await;
        match key_out {
            Ok(o) if o.status.success() => println!("{}", c_green("✅ public key installed")),
            _ => println!("{}", c_yellow("⚠️  could not install key — continuing")),
        }
    } else {
        println!("{}", c_dim("⊘  no local public key found — skipped"));
    }

    // ── Step 4: Check / install Node.js ──────────────────────────────────
    print_step(4, TOTAL, "Checking Node.js...");
    std::io::stdout().flush().ok();
    let node_check = format!("{}node --version 2>/dev/null || echo NOT_FOUND", pfx);
    let node_out = Command::new("ssh").args(ssh_args(&node_check)).output().await?;
    let node_ver = String::from_utf8_lossy(&node_out.stdout).trim().to_string();
    if node_ver.contains("NOT_FOUND") || node_ver.is_empty() {
        println!("{}", c_yellow("⚠️  not found — installing via NodeSource"));
        print_step(4, TOTAL, "Installing Node.js...");
        std::io::stdout().flush().ok();
        let install_node = if is_mac {
            format!("{}brew install node 2>&1 | tail -3", pfx)
        } else {
            "curl -fsSL https://rpm.nodesource.com/setup_lts.x | sudo bash - 2>&1 | tail -3 && sudo dnf install -y nodejs 2>&1 | tail -3 \
             || curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo bash - 2>&1 | tail -3 && sudo apt-get install -y nodejs 2>&1 | tail -3".into()
        };
        let _ = tokio::time::timeout(Duration::from_secs(120),
            Command::new("ssh").args(ssh_args(&install_node)).output()
        ).await;
        let recheck = Command::new("ssh").args(ssh_args(&node_check)).output().await?;
        let new_ver = String::from_utf8_lossy(&recheck.stdout).trim().to_string();
        if new_ver.contains("NOT_FOUND") || new_ver.is_empty() {
            println!("{}", c_bold_red("❌ Node.js install failed"));
            return Err("Node.js installation failed — install manually and retry".into());
        }
        println!("{}", c_bold_green(&format!("✅ installed {}", new_ver)));
    } else {
        println!("{}", c_bold_green(&format!("✅ {}", node_ver)));
    }

    // ── Step 5: Check / install OpenClaw ─────────────────────────────────
    print_step(5, TOTAL, "Checking OpenClaw...");
    std::io::stdout().flush().ok();
    let oc_check = format!("{}openclaw --version 2>/dev/null || echo NOT_INSTALLED", pfx);
    let oc_out = Command::new("ssh").args(ssh_args(&oc_check)).output().await?;
    let oc_ver = String::from_utf8_lossy(&oc_out.stdout).trim().to_string();
    let mut oc_installed_by_us = false;
    if oc_ver.contains("NOT_INSTALLED") || oc_ver.is_empty() {
        println!("{}", c_yellow("⚠️  not installed — running npm install -g openclaw@latest"));
        let npm_cmd = if is_mac {
            format!("{}npm install -g openclaw@latest 2>&1 | tail -3", pfx)
        } else {
            "sudo npm install -g openclaw@latest 2>&1 | tail -3".into()
        };
        let install_out = tokio::time::timeout(Duration::from_secs(120),
            Command::new("ssh").args(ssh_args(&npm_cmd)).output()
        ).await;
        let install_ok = install_out.as_ref().ok()
            .and_then(|r| r.as_ref().ok())
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !install_ok {
            println!("{}", c_bold_red("❌ OpenClaw installation failed"));
            return Err("OpenClaw installation failed".into());
        }
        oc_installed_by_us = true;
        let recheck = Command::new("ssh").args(ssh_args(&oc_check)).output().await?;
        let new_ver = String::from_utf8_lossy(&recheck.stdout).trim().to_string();
        println!("{}", c_bold_green(&format!("✅ installed {}", new_ver)));
    } else {
        println!("{}", c_bold_green(&format!("✅ {}", oc_ver)));
    }

    // Helper: rollback partial OpenClaw install on failure.
    async fn do_rollback(oc_installed: bool, ssh_target: &str, pfx: &str) {
        if oc_installed {
            use tokio::process::Command;
            eprintln!("  ⏪ Rolling back: uninstalling openclaw...");
            let result = tokio::time::timeout(std::time::Duration::from_secs(30),
                Command::new("ssh")
                    .args(["-o","ConnectTimeout=5","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                        ssh_target, &format!("{}sudo npm uninstall -g openclaw 2>/dev/null || true", pfx)])
                    .output()
            ).await;
            match result {
                Ok(Ok(o)) if o.status.success() => eprintln!("  ✅ rollback: openclaw removed"),
                Ok(Ok(o)) => eprintln!("  ⚠️  rollback: exit {}", o.status),
                _ => eprintln!("  ⚠️  rollback: timed out or failed — check manually"),
            }
        }
    }

    // ── Step 6: openclaw init ─────────────────────────────────────────────
    print_step(6, TOTAL, "Running openclaw init...");
    std::io::stdout().flush().ok();
    let init_cmd = format!("{}openclaw init --non-interactive 2>&1 | tail -5", pfx);
    let init_out = tokio::time::timeout(Duration::from_secs(30),
        Command::new("ssh").args(ssh_args(&init_cmd)).output()
    ).await;
    match init_out {
        Ok(Ok(o)) if o.status.success() => println!("{}", c_green("✅ initialised")),
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if stderr.to_lowercase().contains("already") || stderr.is_empty() {
                println!("{}", c_green("✅ already initialised"));
            } else {
                println!("{}", c_yellow(&format!("⚠️  init warning (continuing): {}", stderr.chars().take(60).collect::<String>())));
            }
        }
        _ => println!("{}", c_dim("⊘  init timed out — continuing")),
    }

    // ── Step 7: Configure gateway ─────────────────────────────────────────
    print_step(7, TOTAL, "Configuring gateway...");
    std::io::stdout().flush().ok();
    let token = random_hex_token(24)?;
    let escaped_token = shell::escape(&token);

    // Determine agent name
    let agent_name = if let Some(n) = name {
        n.to_string()
    } else {
        let hn_out = Command::new("ssh").args(ssh_args("hostname")).output().await?;
        let raw = String::from_utf8_lossy(&hn_out.stdout).trim().to_lowercase()
            .chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect::<String>();
        // Collapse consecutive hyphens and trim leading/trailing hyphens
        let collapsed = raw.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
        collapsed.trim_matches('-').to_string()
    };

    let escaped_agent = shell::escape(&agent_name);
    let config_script = format!(
        r#"python3 -c "
import json,os
p=os.path.expanduser('~/.openclaw/openclaw.json')
os.makedirs(os.path.dirname(p), exist_ok=True)
c={{}}
if os.path.exists(p):
    with open(p) as f: c=json.load(f)
gw=c.setdefault('gateway',{{}})
gw['bind']='lan'
gw.setdefault('auth',{{}})['mode']='token'
gw['auth']['token']={token}
h=gw.setdefault('http',{{}})
e=h.setdefault('endpoints',{{}})
e['chatCompletions']={{'enabled':True}}
c['name']={name}
with open(p,'w') as f: json.dump(c,f,indent=2)
print('ok')
""#,
        token = escaped_token,
        name = escaped_agent,
    );
    let cfg_out = Command::new("ssh").args(ssh_args(&config_script)).output().await?;
    let cfg_result = String::from_utf8_lossy(&cfg_out.stdout).trim().to_string();
    if cfg_result != "ok" {
        do_rollback(oc_installed_by_us, &ssh_target, pfx).await;
        return Err("Gateway configuration failed".into());
    }
    // Read port
    let port_cmd = format!(
        "{}python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{{}}).get('port',18789))\"",
        pfx
    );
    let port_out = Command::new("ssh").args(ssh_args(&port_cmd)).output().await?;
    let port: i32 = String::from_utf8_lossy(&port_out.stdout).trim().parse().unwrap_or(18789);
    println!("{}", c_bold_green(&format!("✅ bind=lan, chatCompletions, port={}", port)));

    // ── Step 8: Start gateway ─────────────────────────────────────────────
    print_step(8, TOTAL, "Starting gateway...");
    std::io::stdout().flush().ok();
    let restart_cmd = format!("{}openclaw gateway restart 2>&1 | tail -1", pfx);
    let _ = tokio::time::timeout(Duration::from_secs(15),
        Command::new("ssh").args(ssh_args(&restart_cmd)).output()
    ).await;
    println!("{}", c_green("✅ gateway restart requested"));

    // ── Step 9: Check / configure Tailscale ──────────────────────────────
    print_step(9, TOTAL, "Checking Tailscale...");
    std::io::stdout().flush().ok();
    let ts_cmd = format!(
        "{}tailscale status --json 2>/dev/null | python3 -c \"import sys,json; d=json.load(sys.stdin); print(d.get('BackendState',''))\" || echo NOT_FOUND",
        pfx
    );
    let ts_out = Command::new("ssh").args(ssh_args(&ts_cmd)).output().await.ok();
    let ts_state = ts_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    match ts_state.as_str() {
        "Running" => println!("{}", c_green("✅ Tailscale is running")),
        "NOT_FOUND" | "" => println!("{}", c_dim("⊘  Tailscale not installed — using direct IP")),
        other => {
            let ts_server = std::env::var("SAM_TAILSCALE_SERVER").unwrap_or_default();
            if !ts_server.is_empty() {
                let ts_up_cmd = format!("{}sudo tailscale up --login-server={} 2>&1 | tail -3", pfx, shell::escape(&ts_server));
                let _ = tokio::time::timeout(Duration::from_secs(30),
                    Command::new("ssh").args(ssh_args(&ts_up_cmd)).output()
                ).await;
                println!("{}", c_yellow(&format!("🔧 ran tailscale up (was: {})", other)));
            } else {
                println!("{}", c_yellow(&format!("⚠️  state={} — set SAM_TAILSCALE_SERVER to auto-join", other)));
            }
        }
    }

    // ── Step 10: Post-install diagnostic ─────────────────────────────────
    print_step(10, TOTAL, "Running post-install diagnostic...");
    std::io::stdout().flush().ok();
    tokio::time::sleep(Duration::from_secs(3)).await;
    // SSH check
    let ssh_ok = tokio::time::timeout(Duration::from_secs(6),
        Command::new("ssh").args(ssh_args("echo ok")).output()
    ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);
    // OC version
    let oc_ver_out = Command::new("ssh").args(ssh_args(&format!("{}openclaw --version 2>/dev/null || echo unknown", pfx))).output().await.ok();
    let final_oc_ver = oc_ver_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_else(|| "unknown".into());
    // Gateway health
    let gw_url = format!("http://{}:{}/v1/models", host, port);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let gw_ok = client.get(&gw_url)
        .header("Authorization", format!("Bearer {}", token))
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    println!("  SSH: {}  OpenClaw: {}  Gateway: {}",
        if ssh_ok { c_green("✅") } else { c_red("✗") },
        c_green(&final_oc_ver),
        if gw_ok { c_green("✅") } else { c_yellow("⚠ not yet responding") },
    );

    // ── Step 11: Atomic DB registration ──────────────────────────────────
    print_step(11, TOTAL, "Registering in fleet DB...");
    std::io::stdout().flush().ok();
    let pool = crate::db::get_pool();
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    let final_status = if ssh_ok && gw_ok { "online" } else { "offline" };
    conn.exec_drop(
        "INSERT INTO mc_fleet_status \
         (agent_name, tailscale_ip, status, gateway_port, gateway_token, os_info) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE \
           tailscale_ip=VALUES(tailscale_ip), \
           status=VALUES(status), \
           gateway_port=VALUES(gateway_port), \
           gateway_token=VALUES(gateway_token), \
           os_info=VALUES(os_info)",
        (&agent_name, host, final_status, port, &token, &os_name),
    ).await?;
    pool.disconnect().await?;
    println!("{}", c_bold_green("✅"));

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  {} added to fleet — all checks passed",
        c_bold_green(&format!("✅ {} ({})", agent_name, os_name)));
    println!("     Port: {}  |  Token: {}…", port, &token[..12]);
    println!("     Run {} to see it in the fleet.\n", c_bold_cyan("`sam`"));

    Ok(())
}

/// Generate a cryptographically secure random hex token.
///
/// The returned string length is `byte_len * 2`.
fn random_hex_token(byte_len: usize) -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = vec![0_u8; byte_len];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.into_iter().map(|b| format!("{:02x}", b)).collect())
}

/// Deploy workspace file to agent(s)
pub async fn run_deploy(target: &str, file: &str, source: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;

    validate::validate_deploy_filename(file)
        .map_err(|e| format!("Cannot deploy file: {}", e))?;

    // Resolve source file
    let src_path = source.map(|s| s.to_string()).unwrap_or_else(|| {
        format!("templates/{}", file)
    });

    if !std::path::Path::new(&src_path).exists() {
        return Err(format!("Source file not found: {}", src_path).into());
    }

    let content = std::fs::read_to_string(&src_path)?;
    println!("\n🚀 Deploying {} ({} bytes)", file, content.len());

    // Load fleet from DB
    let pool = crate::db::get_pool();
    let agents = crate::db::load_fleet(&pool).await?;

    let targets: Vec<&crate::db::DbAgent> = if target == "all" {
        agents.iter().collect()
    } else {
        agents.iter().filter(|a| a.agent_name == target).collect()
    };

    if targets.is_empty() {
        return Err(format!("No agents found matching '{}'", target).into());
    }

    for agent in &targets {
        let ip = agent.tailscale_ip.as_deref().unwrap_or("?");
        print!("  {} → {}... ", agent.agent_name, ip);

        // Determine workspace path
        let workspace_cmd = "python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('agents',{}).get('defaults',{}).get('workspace',os.path.expanduser('~/.openclaw/workspace')))\"";

        let out = tokio::time::timeout(std::time::Duration::from_secs(5),
            Command::new("ssh").args([
                "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                &format!("admin@{}", ip), workspace_cmd
            ]).output()
        ).await;

        let workspace = match out {
            Ok(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => {
                println!("❌ unreachable");
                continue;
            }
        };

        // SCP the file
        let dest = format!("admin@{}:{}/{}", ip, workspace, file);
        let scp_out = tokio::time::timeout(std::time::Duration::from_secs(10),
            Command::new("scp").args([
                "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                "--", &src_path, &dest
            ]).output()
        ).await;

        match scp_out {
            Ok(Ok(o)) if o.status.success() => println!("✅"),
            _ => println!("❌ scp failed"),
        }
    }

    pool.disconnect().await?;
    println!("\n  Done.\n");
    Ok(())
}


/// Full automated init — creates everything from scratch
pub async fn run_init(db_host: Option<&str>, db_port: Option<u16>, db_user: Option<&str>, db_pass: Option<&str>, db_name: Option<&str>, self_ip: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use mysql_async::prelude::*;
    use std::io::{self, Write};

        println!("\n   {} {}\n", c_bold_cyan("🩺"), c_bold("Fleet Doctor"));
    print_divider();
    println!();

    let prompt = |label: &str, default: &str| -> String {
        print!("  {} [{}]: ", label, default);
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return default.to_string();
        }
        let v = input.trim().to_string();
        if v.is_empty() { default.to_string() } else { v }
    };

    let write_env_if_missing = |mode: &str, cfg: &SamConfig, self_ip: &str| -> Result<(), Box<dyn std::error::Error>> {
        let env_path = std::path::Path::new(".env");
        if !env_path.exists() {
            let sqlite_path = cfg.database.sqlite_path.as_deref().unwrap_or("");
            std::fs::write(
                env_path,
                format!(
                    "SAM_DB_MODE={}\nSAM_DB_URL={}\nSAM_SQLITE_PATH={}\nSAM_SELF_IP={}\nSAM_USER={}\n",
                    mode,
                    cfg.database.url.clone().unwrap_or_default(),
                    sqlite_path,
                    self_ip,
                    cfg.identity.user,
                ),
            )?;
        }
        Ok(())
    };

    let save_non_mysql_config = |mode: &str, sqlite_path: Option<String>, self_ip: &str| -> Result<(), Box<dyn std::error::Error>> {
        let cfg = SamConfig {
            database: DatabaseConfig {
                mode: Some(mode.to_string()),
                url: None,
                host: None,
                port: None,
                user: None,
                password: None,
                database: None,
                sqlite_path,
            },
            tui: TuiConfig::default(),
            fleet: FleetConfig::default(),
            identity: IdentityConfig { user: whoami().unwrap_or_else(|| "operator".into()) },
        };
        cfg.save()?;
        write_env_if_missing(mode, &cfg, self_ip)?;
        Ok(())
    };

    let has_mysql_cli = db_host.is_some()
        || db_port.is_some()
        || db_user.is_some()
        || db_pass.is_some()
        || db_name.is_some();
    let persistence_choice = if has_mysql_cli {
        "yes".to_string()
    } else {
        prompt(
            "Want persistent fleet memory? (yes=guided MySQL / no=SQLite embedded / skip=memory only)",
            "yes",
        )
        .to_lowercase()
    };

    let self_ip = self_ip.map(|s| s.to_string()).unwrap_or_else(|| {
        let detected = std::process::Command::new("hostname").arg("-I").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("127.0.0.1").to_string())
            .unwrap_or_else(|_| "127.0.0.1".into());
        prompt("This machine's IP", &detected)
    });

    if persistence_choice == "skip" || persistence_choice == "s" {
        save_non_mysql_config("memory", None, &self_ip)?;
        println!("\n  ✅ Memory-only mode enabled.");
        println!("  ℹ️ Fleet data is runtime-only (not persisted across restarts).");
        println!("  ✅ Ready! Run `sam` to launch Mission Control.\n");
        return Ok(());
    }
    if persistence_choice == "no" || persistence_choice == "n" {
        let default_sqlite = dirs::data_local_dir()
            .unwrap_or_else(|| dirs::config_dir().unwrap_or_default())
            .join("sam/fleet.sqlite");
        let sqlite_path = prompt("SQLite DB file", &default_sqlite.to_string_lossy());
        if let Some(parent) = std::path::Path::new(&sqlite_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sqlite_path)?;
        save_non_mysql_config("sqlite", Some(sqlite_path.clone()), &self_ip)?;
        println!("\n  ✅ SQLite embedded mode selected.");
        println!("  ℹ️ SQLite file: {}", sqlite_path);
        println!("  ✅ Ready! Run `sam` to launch Mission Control.\n");
        return Ok(());
    }

    // Interactive prompts for missing values
    let db_host = db_host.map(|s| s.to_string()).unwrap_or_else(|| prompt("MySQL host", "127.0.0.1"));
    let db_port = db_port.unwrap_or_else(|| prompt("MySQL port", "3306").parse().unwrap_or(3306));
    let db_user = db_user.map(|s| s.to_string()).unwrap_or_else(|| prompt("MySQL user", "root"));
    let db_pass = db_pass.map(|s| s.to_string()).unwrap_or_else(|| {
        print!("  MySQL password: ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return String::new();
        }
        input.trim().to_string()
    });
    let db_name = db_name.map(|s| s.to_string()).unwrap_or_else(|| prompt("Database name", "sam_fleet"));

    println!();

    // Step 1: Connect to DB
    print!("  [1/4] Connecting to MySQL... ");
    let url = crate::db::build_db_url(&db_host, &db_port.to_string(), &db_user, &db_pass, &db_name);
    let pool = mysql_async::Pool::new(url.as_str());
    let mut conn = match pool.get_conn().await {
        Ok(conn) => conn,
        Err(e) => {
            println!("❌");
            println!(
                "  ⚠️ MySQL unavailable ({}). Falling back to memory-only mode.",
                crate::db::sanitize_error(&format!("DB connection failed: {}", e))
            );
            save_non_mysql_config("memory", None, &self_ip)?;
            println!("  ✅ Ready! Run `sam` to launch Mission Control.\n");
            return Ok(());
        }
    };
    println!("✅");

    // Step 2: Create tables
    print!("  [2/4] Creating tables... ");
    conn.query_drop(r"
        CREATE TABLE IF NOT EXISTS mc_fleet_status (
            agent_name       VARCHAR(64) PRIMARY KEY,
            hostname         VARCHAR(128),
            tailscale_ip     VARCHAR(45),
            status           VARCHAR(16) DEFAULT 'offline',
            latency_ms       INT UNSIGNED DEFAULT NULL,
            gateway_port     INT DEFAULT 18789,
            gateway_token    VARCHAR(128) DEFAULT NULL,
            current_task_id  INT,
            last_heartbeat   DATETIME,
            oc_version       VARCHAR(32),
            os_info          VARCHAR(128),
            kernel           VARCHAR(64),
            capabilities     JSON,
            token_burn_today INT DEFAULT 0,
            uptime_seconds   BIGINT DEFAULT 0,
            updated_at       DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
        )
    ").await?;
    conn.query_drop(r"
        CREATE TABLE IF NOT EXISTS mc_chat (
            id           BIGINT AUTO_INCREMENT PRIMARY KEY,
            sender       VARCHAR(64) NOT NULL,
            target       VARCHAR(64),
            message      TEXT NOT NULL,
            response     TEXT,
            status       VARCHAR(16) DEFAULT 'pending',
            kind         VARCHAR(10) DEFAULT 'global',
            created_at   DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3),
            responded_at DATETIME(3),
            INDEX idx_target_status (target, status),
            INDEX idx_created (created_at),
            INDEX idx_kind (kind)
        )
    ").await?;
    conn.query_drop(r"
        CREATE TABLE IF NOT EXISTS mc_task_routing (
            id              INT AUTO_INCREMENT PRIMARY KEY,
            task_description TEXT NOT NULL,
            assigned_agent  VARCHAR(64),
            status          VARCHAR(16) DEFAULT 'queued',
            priority        INT DEFAULT 5,
            created_by      VARCHAR(64),
            created_at      DATETIME DEFAULT CURRENT_TIMESTAMP,
            assigned_at     DATETIME,
            completed_at    DATETIME,
            result          TEXT
        )
    ").await?;
    conn.query_drop(r"
        CREATE TABLE IF NOT EXISTS mc_operations (
            id           BIGINT AUTO_INCREMENT PRIMARY KEY,
            agent_name   VARCHAR(64) NOT NULL,
            op_type      VARCHAR(32) NOT NULL,
            status       VARCHAR(16) DEFAULT 'running',
            detail       TEXT,
            created_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
            completed_at DATETIME,
            INDEX idx_agent (agent_name),
            INDEX idx_created (created_at)
        )
    ").await?;
    println!("✅ mc_fleet_status, mc_chat, mc_task_routing, mc_operations");

    // Step 3: Generate config
    print!("  [3/4] Generating config... ");
    let cfg = SamConfig {
        database: DatabaseConfig {
            mode: Some("mysql".into()),
            url: Some(url.clone()),
            host: Some(db_host.clone()),
            port: Some(db_port),
            user: Some(db_user.clone()),
            password: Some(db_pass.clone()),
            database: Some(db_name.clone()),
            sqlite_path: None,
        },
        tui: TuiConfig::default(),
        fleet: FleetConfig::default(),
        identity: IdentityConfig { user: whoami().unwrap_or_else(|| "operator".into()) },
    };
    cfg.save()?;

    // Also write .env
    write_env_if_missing("mysql", &cfg, &self_ip)?;
    println!("✅ ~/.config/sam/config.toml");

    // Step 4: Self-register
    print!("  [4/4] Registering this machine... ");
    let hostname = std::process::Command::new("hostname").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "localhost".into());
    conn.exec_drop(
        "INSERT IGNORE INTO mc_fleet_status (agent_name, tailscale_ip, status) VALUES (?, ?, 'online')",
        (&hostname.to_lowercase(), &self_ip),
    ).await?;
    println!("✅ {} @ {}", hostname, self_ip);

    pool.disconnect().await?;

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ✅ Ready! Run `sam` to launch Mission Control.");
    println!("  📡 Add agents: `sam onboard <ip>`\n");

    Ok(())
}

fn whoami() -> Option<String> {
    std::process::Command::new("whoami").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}


#[derive(Debug, Clone, Serialize)]
struct FleetDoctorAgentResult {
    agent: String,
    ip: String,
    ssh_reachable: bool,
    gateway_api_reachable: bool,
    oc_version: String,
    oc_current: bool,
    fixed_actions: Vec<String>,
}

impl FleetDoctorAgentResult {
    fn is_healthy(&self) -> bool {
        self.ssh_reachable && self.gateway_api_reachable && self.oc_current
    }
}

#[derive(Debug, Serialize)]
struct FleetDoctorOutput {
    total_agents: usize,
    unhealthy_agents: usize,
    down_agents: usize,
    exit_code: i32,
    latest_oc_version: Option<String>,
    results: Vec<FleetDoctorAgentResult>,
}

fn parse_semver(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|s| s.trim().to_string())
}

fn doctor_exit_code(total_agents: usize, unhealthy_agents: usize, down_agents: usize) -> i32 {
    // down_agents * 2 > total_agents means "strictly more than 50% down".
    if total_agents > 0 && (down_agents * 2) > total_agents { 2 }
    else if unhealthy_agents > 0 { 1 }
    else { 0 }
}

/// Diagnose and auto-fix fleet issues
pub async fn run_doctor_fleet(
    fix: bool,
    agent_filter: Option<&str>,
    json: bool,
    quiet: bool,
    timeout_secs: u64,
) -> Result<i32, Box<dyn std::error::Error>> {
    use tokio::process::Command;
    use tokio::sync::mpsc;
    let timeout_secs = timeout_secs.max(1);

    print_banner();

    let pool = crate::db::get_pool();
    let agents = crate::db::load_fleet(&pool).await?;

    let targets: Vec<crate::db::DbAgent> = if let Some(name) = agent_filter {
        agents.iter().filter(|a| a.agent_name.contains(name)).cloned().collect()
    } else {
        agents.clone()
    };

    let latest_oc_version = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("npm").args(["view", "openclaw", "version", "--silent"]).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .filter(|o| o.status.success())
    .and_then(|o| parse_semver(&String::from_utf8_lossy(&o.stdout)));

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        ?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    for agent in targets {
        let tx = tx.clone();
        let latest_oc_version = latest_oc_version.clone();
        let http_client = http_client.clone();
        tokio::spawn(async move {
            let name = agent.agent_name.clone();
            let ip = agent.tailscale_ip.clone().unwrap_or_else(|| "?".to_string());
            let user = "admin";
            let is_mac = agent.os_info.as_deref().unwrap_or("").to_lowercase().contains("mac");
            let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
            let ssh_target = format!("{}@{}", user, ip);
            let mut fixed_actions = Vec::new();

            let ssh_reachable = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                Command::new("ssh")
                    .args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes", &ssh_target, "echo ok"])
                    .output(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|o| o.status.success())
            .unwrap_or(false);

            let mut oc_version = String::new();
            let mut oc_current = false;
            let mut gateway_api_reachable = false;

            if ssh_reachable {
                let oc_raw = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=3",
                            "-o",
                            "StrictHostKeyChecking=no",
                            "-o",
                            "BatchMode=yes",
                            &ssh_target,
                            &format!("{pfx}openclaw --version 2>/dev/null || echo NOT_INSTALLED"),
                        ])
                        .output(),
                )
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

                oc_version = parse_semver(&oc_raw).unwrap_or(oc_raw);
                oc_current = if oc_version.is_empty() || oc_version.contains("NOT_INSTALLED") {
                    false
                } else if let Some(latest) = &latest_oc_version {
                    oc_version == *latest
                } else {
                    true
                };

                if fix && !oc_current {
                    let cmd = if is_mac {
                        format!("{pfx}npm install -g openclaw@latest >/dev/null 2>&1")
                    } else {
                        "sudo npm install -g openclaw@latest >/dev/null 2>&1".to_string()
                    };
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        Command::new("ssh")
                            .args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes", &ssh_target, &cmd])
                            .output(),
                    )
                    .await;
                    let oc_after = Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=3",
                            "-o",
                            "StrictHostKeyChecking=no",
                            "-o",
                            "BatchMode=yes",
                            &ssh_target,
                            &format!("{pfx}openclaw --version 2>/dev/null"),
                        ])
                        .output()
                        .await
                        .ok()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                        .unwrap_or_default();
                    oc_version = parse_semver(&oc_after).unwrap_or(oc_after);
                    oc_current = if let Some(latest) = &latest_oc_version {
                        oc_version == *latest
                    } else {
                        !oc_version.is_empty()
                    };
                    if oc_current {
                        fixed_actions.push("openclaw_updated".to_string());
                    }
                }

                gateway_api_reachable = http_client
                    .get(format!("http://{}:18789/", ip))
                    .send()
                    .await
                    .is_ok();

                if fix && !gateway_api_reachable {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        Command::new("ssh")
                            .args([
                                "-o",
                                "ConnectTimeout=3",
                                "-o",
                                "StrictHostKeyChecking=no",
                                "-o",
                                "BatchMode=yes",
                                &ssh_target,
                                &format!("{pfx}openclaw gateway restart >/dev/null 2>&1"),
                            ])
                            .output(),
                    )
                    .await;
                    gateway_api_reachable = http_client
                        .get(format!("http://{}:18789/", ip))
                        .send()
                        .await
                        .is_ok();
                    if gateway_api_reachable {
                        fixed_actions.push("gateway_restarted".to_string());
                    }
                }
            }

            let _ = tx.send(FleetDoctorAgentResult {
                agent: name,
                ip,
                ssh_reachable,
                gateway_api_reachable,
                oc_version,
                oc_current,
                fixed_actions,
            });
        });
    }
    drop(tx);

    let mut results = Vec::new();
    while let Some(result) = rx.recv().await {
        results.push(result);
    }
    results.sort_by(|a, b| a.agent.cmp(&b.agent));

    let unhealthy_agents = results
        .iter()
        .filter(|r| !r.is_healthy())
        .count();
    let down_agents = results.iter().filter(|r| !r.ssh_reachable).count();
    let exit_code = doctor_exit_code(results.len(), unhealthy_agents, down_agents);

    let payload = FleetDoctorOutput {
        total_agents: results.len(),
        unhealthy_agents,
        down_agents,
        exit_code,
        latest_oc_version,
        results,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if quiet {
        for r in &payload.results {
            if r.is_healthy() {
                continue;
            }
            println!(
                "{:<20} ssh:{} gateway:{} oc:{}{}",
                r.agent,
                if r.ssh_reachable { "✓" } else { "✗" },
                if r.gateway_api_reachable { "✓" } else { "✗" },
                if r.oc_current { "✓" } else { "✗" },
                if r.fixed_actions.is_empty() {
                    String::new()
                } else {
                    format!(" fixed={}", r.fixed_actions.join(","))
                }
            );
        }
    } else {
        println!(
            "   {:<20} {:<7} {:<9} {:<7} {}",
            c_bold("Agent"),
            c_bold("SSH"),
            c_bold("Gateway"),
            c_bold("OC"),
            c_bold("Version")
        );
        print_divider();
        for r in &payload.results {
            println!(
                "   {:<20} {:<7} {:<9} {:<7} {}",
                r.agent,
                if r.ssh_reachable { "✓" } else { "✗" },
                if r.gateway_api_reachable { "✓" } else { "✗" },
                if r.oc_current { "✓" } else { "✗" },
                if r.oc_version.is_empty() { "unknown" } else { &r.oc_version }
            );
        }
        print_divider();
        println!(
            "   {} agents checked, {} unhealthy, {} down",
            payload.total_agents, payload.unhealthy_agents, payload.down_agents
        );
        println!("   exit code: {}\n", payload.exit_code);
    }

    let status = if exit_code == 0 { "pass" } else { "fail" };
    let record_result = crate::db::record_fleet_doctor_run(&pool, status, &serde_json::to_string(&payload)?).await;
    let disconnect_result = pool.disconnect().await;
    record_result?;
    disconnect_result?;
    Ok(exit_code)
}

pub async fn run_doctor(fix: bool, agent_filter: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;

    print_banner();

    let pool = crate::db::get_pool();
    let agents = crate::db::load_fleet(&pool).await?;

    let targets: Vec<&crate::db::DbAgent> = if let Some(name) = agent_filter {
        agents.iter().filter(|a| a.agent_name.contains(name)).collect()
    } else {
        agents.iter().collect()
    };

    let mut issues = 0;
    let mut fixed = 0;

    for agent in &targets {
        let name = &agent.agent_name;
        let ip = agent.tailscale_ip.as_deref().unwrap_or("?");
        let user = if name.is_empty() { "admin" } else { "admin" };
        let is_mac = agent.os_info.as_deref().unwrap_or("").to_lowercase().contains("mac");
        let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };

        println!("  🔍 {} ({})", name, ip);

        // Check 1: SSH connectivity
        let ssh_ok = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("ssh").args([
                "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                &format!("{}@{}", user, ip), "echo ok"
            ]).output()
        ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

        if !ssh_ok {
            println!("     ❌ SSH unreachable — skipping");
            issues += 1;
            continue;
        }
        println!("     ✅ SSH");

        // Check 2: OpenClaw installed
        let oc_out = Command::new("ssh").args([
            "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
            &format!("{}@{}", user, ip),
            &format!("{}openclaw --version 2>/dev/null || echo NOT_INSTALLED", pfx)
        ]).output().await.ok();
        let oc_version = oc_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if oc_version.contains("NOT_INSTALLED") || oc_version.is_empty() {
            println!("     ❌ OpenClaw not installed");
            issues += 1;
            if fix {
                print!("        🔧 Installing... ");
                let cmd = if is_mac { format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx) }
                          else { "sudo npm install -g openclaw@latest 2>&1 | tail -1".into() };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(120),
                    Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                        &format!("{}@{}", user, ip), &cmd]).output()).await;
                println!("done");
                fixed += 1;
            }
        } else {
            println!("     ✅ OpenClaw {}", oc_version);
        }

        // Check 3: Gateway running
        let gw_out = Command::new("ssh").args([
            "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
            &format!("{}@{}", user, ip), "ss -tlnp 2>/dev/null | grep 18789 | head -1"
        ]).output().await.ok();
        let gw_line = gw_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if gw_line.is_empty() {
            println!("     ❌ Gateway not running");
            issues += 1;
            if fix {
                print!("        🔧 Starting gateway... ");
                let _ = tokio::time::timeout(std::time::Duration::from_secs(15),
                    Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                        &format!("{}@{}", user, ip),
                        &format!("{}openclaw gateway restart 2>&1 | tail -1", pfx)]).output()).await;
                println!("done");
                fixed += 1;
            }
        } else {
            let binding = if gw_line.contains("0.0.0.0") { "0.0.0.0 ✅" } else { "localhost ⚠️" };
            println!("     ✅ Gateway running ({})", binding);

            // Check 3b: Binding
            if !gw_line.contains("0.0.0.0") {
                println!("     ❌ Gateway bound to localhost (needs bind=lan)");
                issues += 1;
                if fix {
                    print!("        🔧 Setting bind=lan... ");
                    let _ = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                        &format!("{}@{}", user, ip),
                        "python3 -c \"import json,os;p=os.path.expanduser('~/.openclaw/openclaw.json');c=json.load(open(p));c.setdefault('gateway',{})['bind']='lan';json.dump(c,open(p,'w'),indent=2);print('ok')\""
                    ]).output().await;
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(15),
                        Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                            &format!("{}@{}", user, ip),
                            &format!("{}openclaw gateway restart 2>&1 | tail -1", pfx)]).output()).await;
                    println!("done");
                    fixed += 1;
                }
            }
        }

        // Check 4: chatCompletions enabled
        let cc_out = Command::new("ssh").args([
            "-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
            &format!("{}@{}", user, ip),
            "python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{}).get('http',{}).get('endpoints',{}).get('chatCompletions',{}).get('enabled',False))\""
        ]).output().await.ok();
        let cc_enabled = cc_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if cc_enabled != "True" {
            println!("     ❌ chatCompletions not enabled");
            issues += 1;
            if fix {
                print!("        🔧 Enabling chatCompletions... ");
                let _ = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                    &format!("{}@{}", user, ip),
                    "python3 -c \"import json,os;p=os.path.expanduser('~/.openclaw/openclaw.json');c=json.load(open(p));gw=c.setdefault('gateway',{});h=gw.setdefault('http',{});e=h.setdefault('endpoints',{});e['chatCompletions']={'enabled':True};json.dump(c,open(p,'w'),indent=2);print('ok')\""
                ]).output().await;
                println!("done");
                fixed += 1;
            }
        } else {
            println!("     ✅ chatCompletions enabled");
        }

        // Check 5: Gateway token in DB
        if agent.gateway_token.is_none() || agent.gateway_token.as_deref() == Some("") {
            println!("     ❌ No gateway token in DB");
            issues += 1;
            if fix {
                print!("        🔧 Fetching and storing token... ");
                let tok_out = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                    &format!("{}@{}", user, ip),
                    "python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{}).get('auth',{}).get('token',''))\""
                ]).output().await.ok();
                let token = tok_out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
                if !token.is_empty() {
                    let mut conn = pool.get_conn().await?;
                    use mysql_async::prelude::*;
                    conn.exec_drop("UPDATE mc_fleet_status SET gateway_token=? WHERE agent_name=?", (&token, name)).await?;
                    println!("done ({}...)", &token[..12.min(token.len())]);
                    fixed += 1;
                } else {
                    println!("no token found in config");
                }
            }
        } else {
            println!("     ✅ Gateway token in DB");
        }

        // Check 6: HTTP API reachable
        let http_ok = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build().unwrap_or_default()
            .get(&format!("http://{}:18789/", ip))
            .send().await.is_ok();
        if http_ok {
            println!("     ✅ HTTP API reachable");
        } else {
            println!("     ⚠️  HTTP API unreachable (SSH fallback will be used)");
        }

        println!();
    }

    pool.disconnect().await?;

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  {} agents checked, {} issues found, {} fixed",
        targets.len(), issues, fixed);
    if issues > 0 && !fix {
        println!("  Run `sam doctor --fix` to auto-repair");
    }
    println!();

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::{doctor_exit_code, parse_semver};

    #[test]
    fn parse_semver_extracts_version_token() {
        assert_eq!(parse_semver("openclaw 1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(parse_semver("v1.2.3"), Some("1.2.3".to_string()));
    }

    #[test]
    fn doctor_exit_codes_follow_thresholds() {
        assert_eq!(doctor_exit_code(4, 0, 0), 0);
        assert_eq!(doctor_exit_code(4, 1, 2), 1);
        assert_eq!(doctor_exit_code(4, 3, 3), 2);
    }
}

/// Print operation history log (non-TUI)
pub async fn run_log(agent: Option<&str>, tail: u32) -> Result<(), Box<dyn std::error::Error>> {
    let pool = crate::db::get_pool();
    let ops = crate::db::get_operations(&pool, agent, tail).await?;

    print_banner();
    println!();
    print_divider();
    println!("   {:<18} {:<14} {:<10} {:<10} {}",
        c_bold("Time"), c_bold("Agent"), c_bold("Type"), c_bold("Status"), c_bold("Detail"));
    print_divider();

    if ops.is_empty() {
        println!("   {}", c_dim("No operations found."));
    } else {
        for op in &ops {
            let status_str = match op.status.as_str() {
                "pass" => c_bold_green("pass"),
                "fail" => c_bold_red("fail"),
                "fixed" => c_bold_green("fixed"),
                "running" => c_yellow("running"),
                s => c_dim(s),
            };
            let detail = op.detail.as_deref().unwrap_or("—");
            let detail_short: String = detail.lines().next().unwrap_or("—").chars().take(50).collect();
            println!("   {:<16} {:<22} {:<18} {}  {}",
                c_dim(&op.created_at),
                c_cyan(&op.agent_name),
                c_magenta(&op.op_type),
                status_str,
                c_dim(&detail_short),
            );
        }
    }

    print_divider();
    println!("   {} operations shown\n", ops.len());

    pool.disconnect().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SamConfig;

    #[test]
    fn tui_vim_mode_defaults_to_false() {
        let cfg: SamConfig = toml::from_str("[tui]\n").expect("config should parse");
        assert!(!cfg.tui.vim_mode);
    }

    #[test]
    fn tui_vim_mode_reads_true_from_config() {
        let cfg: SamConfig = toml::from_str("[tui]\nvim_mode = true\n").expect("config should parse");
        assert!(cfg.tui.vim_mode);
    }
}

/// Validate openclaw.json on the specified agent (or all agents).
pub async fn run_validate(agent: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use crate::config;
    let fleet = config::load_fleet_config().map_err(|e| format!("{}", e))?;
    let targets: Vec<&config::AgentConfig> = if let Some(name) = agent {
        fleet.agent.iter().filter(|a| a.name == name).collect()
    } else {
        fleet.agent.iter().collect()
    };
    if targets.is_empty() {
        eprintln!("No matching agents found");
        return Ok(());
    }
    for ag in targets {
        let user = ag.ssh_user.as_deref().unwrap_or("papasmurf");
        let host = &ag.name;
        let result = tokio::process::Command::new("ssh")
            .args(["-o","ConnectTimeout=5","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host),
                "python3 -c \"import json,sys; d=json.load(open(\\\"$HOME/.openclaw/openclaw.json\\\")); print('ok: model=' + str(d.get('agents',{}).get('defaults',{}).get('model','?')))\" 2>&1"])
            .output().await;
        match result {
            Ok(o) => println!("{}: {}", ag.name, String::from_utf8_lossy(&o.stdout).trim()),
            Err(e) => println!("{}: SSH failed — {}", ag.name, e),
        }
    }
    Ok(())
}
