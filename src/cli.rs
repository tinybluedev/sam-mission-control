use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Run interactive setup wizard
    Setup,
    /// Print version info
    Version,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self { url: None, host: None, port: None, user: None, password: None, database: None }
    }
}

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
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { theme: default_theme(), background: default_bg(), refresh_interval: default_refresh(), chat_poll_interval: default_chat_poll() }
    }
}

fn default_theme() -> String { "standard".into() }
fn default_bg() -> String { "dark".into() }
fn default_refresh() -> u64 { 30 }
fn default_chat_poll() -> u64 { 3 }

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct FleetConfig {
    pub config_path: Option<String>,
}

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
    print!("  Database name [quantum_memory]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let db = input.trim();
    cfg.database.database = Some(if db.is_empty() { "quantum_memory".into() } else { db.into() });

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

    println!("\n🛰️  S.A.M Fleet Status");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("{:<20} {:<12} {:<16} {}", "Agent", "Status", "Version", "IP");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let mut online = 0;
    for a in &agents {
        let icon = match a.status.as_str() {
            "online" => { online += 1; "●" },
            "busy" => { online += 1; "◉" },
            "offline" | "error" => "○",
            _ => "?",
        };
        println!("{:<20} {} {:<9} {:<16} {}",
            a.agent_name,
            icon,
            a.status,
            a.oc_version.as_deref().unwrap_or("?"),
            a.tailscale_ip.as_deref().unwrap_or("?"),
        );
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("{}/{} online\n", online, agents.len());

    pool.disconnect().await?;
    Ok(())
}

/// Send a chat message and wait for response (non-TUI)
pub async fn send_chat(agent: &str, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = crate::db::get_pool();
    let id = crate::db::send_direct(&pool, "cli", agent, message).await?;
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
