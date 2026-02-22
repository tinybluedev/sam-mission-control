use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── ANSI Color Helpers ──────────────────────────────────────────
fn c_cyan(s: &str) -> String {
    format!("\x1b[36m{}\x1b[0m", s)
}
fn c_green(s: &str) -> String {
    format!("\x1b[32m{}\x1b[0m", s)
}
fn c_red(s: &str) -> String {
    format!("\x1b[31m{}\x1b[0m", s)
}
fn c_yellow(s: &str) -> String {
    format!("\x1b[33m{}\x1b[0m", s)
}
#[allow(dead_code)]
fn c_magenta(s: &str) -> String {
    format!("\x1b[35m{}\x1b[0m", s)
}
fn c_bold(s: &str) -> String {
    format!("\x1b[1m{}\x1b[0m", s)
}
fn c_dim(s: &str) -> String {
    format!("\x1b[2m{}\x1b[0m", s)
}
fn c_bold_cyan(s: &str) -> String {
    format!("\x1b[1;36m{}\x1b[0m", s)
}
fn c_bold_green(s: &str) -> String {
    format!("\x1b[1;32m{}\x1b[0m", s)
}
fn c_bold_red(s: &str) -> String {
    format!("\x1b[1;31m{}\x1b[0m", s)
}
fn c_bold_yellow(s: &str) -> String {
    format!("\x1b[1;33m{}\x1b[0m", s)
}
#[allow(dead_code)]
fn c_bold_magenta(s: &str) -> String {
    format!("\x1b[1;35m{}\x1b[0m", s)
}
#[allow(dead_code)]
fn c_bg_green(s: &str) -> String {
    format!("\x1b[42;30m {}  \x1b[0m", s)
}
#[allow(dead_code)]
fn c_bg_red(s: &str) -> String {
    format!("\x1b[41;37m {}  \x1b[0m", s)
}
#[allow(dead_code)]
fn c_bg_cyan(s: &str) -> String {
    format!("\x1b[46;30m {}  \x1b[0m", s)
}

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
    println!(
        "   {}",
        c_dim("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
    );
}

#[allow(dead_code)]
fn print_step(n: usize, total: usize, msg: &str) {
    print!("   {} {} ", c_bold_cyan(&format!("[{}/{}]", n, total)), msg);
}

#[derive(Parser)]
#[command(
    name = "sam",
    version,
    about = "S.A.M Mission Control — Fleet orchestration TUI"
)]
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
}

/// Persistent config file (~/.config/sam/config.toml)
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DatabaseConfig {
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
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
        Self {
            theme: default_theme(),
            background: default_bg(),
            refresh_interval: default_refresh(),
            chat_poll_interval: default_chat_poll(),
        }
    }
}

fn default_theme() -> String {
    "standard".into()
}
fn default_bg() -> String {
    "dark".into()
}
fn default_refresh() -> u64 {
    30
}
fn default_chat_poll() -> u64 {
    3
}

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
    fn default() -> Self {
        Self {
            user: default_user(),
        }
    }
}

fn default_user() -> String {
    "operator".into()
}

impl SamConfig {
    /// Load config from file, or return defaults
    pub fn load(path: Option<&PathBuf>) -> Self {
        // Explicit path
        if let Some(p) = path
            && let Ok(content) = std::fs::read_to_string(p)
            && let Ok(cfg) = toml::from_str(&content)
        {
            return cfg;
        }

        // Default paths
        let candidates = vec![
            PathBuf::from("config.toml"),
            dirs::config_dir()
                .unwrap_or_default()
                .join("sam/config.toml"),
        ];

        for p in candidates {
            if let Ok(content) = std::fs::read_to_string(&p)
                && let Ok(cfg) = toml::from_str(&content)
            {
                return cfg;
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
            if let Some(url) = &self.database.url
                && std::env::var("SAM_DB_URL").is_err()
            {
                std::env::set_var("SAM_DB_URL", url);
            }
            if let Some(h) = &self.database.host
                && std::env::var("SAM_DB_HOST").is_err()
            {
                std::env::set_var("SAM_DB_HOST", h);
            }
            if let Some(p) = &self.database.port
                && std::env::var("SAM_DB_PORT").is_err()
            {
                std::env::set_var("SAM_DB_PORT", p.to_string());
            }
            if let Some(u) = &self.database.user
                && std::env::var("SAM_DB_USER").is_err()
            {
                std::env::set_var("SAM_DB_USER", u);
            }
            if let Some(p) = &self.database.password
                && std::env::var("SAM_DB_PASS").is_err()
            {
                std::env::set_var("SAM_DB_PASS", p);
            }
            if let Some(d) = &self.database.database
                && std::env::var("SAM_DB_NAME").is_err()
            {
                std::env::set_var("SAM_DB_NAME", d);
            }
            if std::env::var("SAM_USER").is_err() {
                std::env::set_var("SAM_USER", &self.identity.user);
            }
        }
    }

    /// Resolve theme name to ThemeName enum
    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
    cfg.database.host = Some(if host.is_empty() {
        "127.0.0.1".into()
    } else {
        host.into()
    });

    input.clear();
    print!("  MySQL port [3306]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let port = input.trim();
    cfg.database.port = Some(if port.is_empty() {
        3306
    } else {
        port.parse().unwrap_or(3306)
    });

    input.clear();
    print!("  MySQL user [root]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let user = input.trim();
    cfg.database.user = Some(if user.is_empty() {
        "root".into()
    } else {
        user.into()
    });

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
    cfg.database.database = Some(if db.is_empty() {
        "sam_fleet".into()
    } else {
        db.into()
    });

    // Identity
    println!("\n━━━ Identity ━━━");
    input.clear();
    print!("  Your display name [operator]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let name = input.trim();
    cfg.identity.user = if name.is_empty() {
        "operator".into()
    } else {
        name.into()
    };

    // Theme
    println!("\n━━━ Theme ━━━");
    println!("  Available: standard, noir, paper, 1977, 2077, matrix, sunset, arctic");
    input.clear();
    print!("  Default theme [standard]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut input)?;
    let theme = input.trim();
    cfg.tui.theme = if theme.is_empty() {
        "standard".into()
    } else {
        theme.into()
    };

    // Save
    println!();
    match cfg.save() {
        Ok(_) => {
            let path = dirs::config_dir()
                .unwrap_or_default()
                .join("sam/config.toml");
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
    println!(
        "   {:<22} {:<14} {:<18} {}",
        c_bold("Agent"),
        c_bold("Status"),
        c_bold("Version"),
        c_bold("IP")
    );
    print_divider();

    let mut online = 0;
    for a in &agents {
        let (icon, status_str) = match a.status.as_str() {
            "online" => {
                online += 1;
                (c_green("●"), c_green("online"))
            }
            "busy" => {
                online += 1;
                (c_yellow("◉"), c_yellow("busy"))
            }
            "offline" => (c_red("○"), c_red("offline")),
            "error" => (c_bold_red("✖"), c_bold_red("error")),
            _ => (c_dim("?"), c_dim("unknown")),
        };
        let ver = a.oc_version.as_deref().unwrap_or("?");
        let ver_str = if ver.starts_with("2026.2.21") {
            c_green(ver)
        } else if ver == "?" {
            c_dim(ver)
        } else {
            c_yellow(ver)
        };
        println!(
            "   {:<20} {} {:<22} {:<26} {}",
            c_cyan(&a.agent_name),
            icon,
            status_str,
            ver_str,
            c_dim(a.tailscale_ip.as_deref().unwrap_or("?")),
        );
    }

    print_divider();
    let summary = if online == agents.len() {
        c_bold_green(&format!(
            "   ✔ {}/{} online — all systems nominal",
            online,
            agents.len()
        ))
    } else {
        c_bold_yellow(&format!(
            "   ⚠ {}/{} online — {} offline",
            online,
            agents.len(),
            agents.len() - online
        ))
    };
    println!("{}\n", summary);

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
        if let Ok(msgs) = crate::db::load_agent_chat(&pool, agent, 1).await
            && let Some(m) = msgs.last()
            && m.id == id
            && m.status == "responded"
        {
            println!("↳ {}", m.response.as_deref().unwrap_or("(no response)"));
            pool.disconnect().await?;
            return Ok(());
        }
    }

    println!("⏳ Timed out after 30s — check `sam` TUI for response");
    pool.disconnect().await?;
    Ok(())
}

/// Onboard a new agent on a remote machine
pub async fn run_onboard(
    host: &str,
    user: &str,
    name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use tokio::process::Command;

    println!("\n🛰️  S.A.M Mission Control — Agent Onboarding");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Target:  {}@{}", user, host);
    println!();

    let ssh_target = format!("{}@{}", user, host);
    let ssh_args = |cmd: &str| -> Vec<String> {
        vec![
            "-o".into(),
            "ConnectTimeout=5".into(),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            ssh_target.clone(),
            cmd.into(),
        ]
    };

    // Step 1: Test SSH connectivity
    print!("  [1/8] Testing SSH connection... ");
    let out = tokio::time::timeout(
        Duration::from_secs(8),
        Command::new("ssh").args(ssh_args("hostname")).output(),
    )
    .await;
    match out {
        Ok(Ok(o)) if o.status.success() => {
            let hostname = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("✅ {}", hostname);
        }
        _ => {
            println!("❌ Cannot reach {}@{}", user, host);
            return Err("SSH connection failed".into());
        }
    }

    // Step 2: Check OS
    print!("  [2/8] Detecting OS... ");
    let out = Command::new("ssh").args(ssh_args(
        ". /etc/os-release 2>/dev/null && echo \"$PRETTY_NAME\" || sw_vers -productName 2>/dev/null || echo unknown"
    )).output().await?;
    let os_name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let is_mac = os_name.to_lowercase().contains("mac");
    println!("✅ {}", os_name);

    // Step 3: Check if OpenClaw is installed
    print!("  [3/8] Checking OpenClaw... ");
    let pfx = if is_mac {
        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
    } else {
        ""
    };
    let out = Command::new("ssh")
        .args(ssh_args(&format!(
            "{}openclaw --version 2>/dev/null || echo NOT_INSTALLED",
            pfx
        )))
        .output()
        .await?;
    let oc_version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if oc_version.contains("NOT_INSTALLED") {
        println!("⚠️  Not installed — installing...");
        print!("       Installing OpenClaw... ");
        let install_cmd = if is_mac {
            format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx)
        } else {
            "sudo npm install -g openclaw@latest 2>&1 | tail -1".into()
        };
        let out = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("ssh").args(ssh_args(&install_cmd)).output(),
        )
        .await;
        match out {
            Ok(Ok(o)) if o.status.success() => println!("✅"),
            _ => {
                println!("❌ Install failed");
                return Err("OpenClaw installation failed".into());
            }
        }
    } else {
        println!("✅ {}", oc_version);
    }

    // Step 4: Get/generate hostname for agent name
    let agent_name = if let Some(n) = name {
        n.to_string()
    } else {
        let out = Command::new("ssh")
            .args(ssh_args("hostname"))
            .output()
            .await?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .to_lowercase()
            .replace(' ', "-")
    };
    println!("  [4/8] Agent name: {}", agent_name);

    // Step 5: Generate auth token
    print!("  [5/8] Configuring gateway... ");
    let token: String = (0..24).map(|_| format!("{:02x}", rand_byte())).collect();
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
gw['auth']['token']='{}'
h=gw.setdefault('http',{{}})
e=h.setdefault('endpoints',{{}})
e['chatCompletions']={{'enabled':True}}
with open(p,'w') as f: json.dump(c,f,indent=2)
print('ok')
""#,
        token
    );
    let out = Command::new("ssh")
        .args(ssh_args(&config_script))
        .output()
        .await?;
    let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if result == "ok" {
        println!("✅ bind=lan, chatCompletions, auth token");
    } else {
        println!("⚠️  {}", result);
    }

    // Step 6: Get gateway port
    print!("  [6/8] Reading gateway port... ");
    let out = Command::new("ssh").args(ssh_args(&format!(
        "{}python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{{}}).get('port',18789))\"", pfx
    ))).output().await?;
    let port: i32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(18789);
    println!("✅ {}", port);

    // Step 7: Register in DB
    print!("  [7/8] Registering in fleet DB... ");
    let pool = crate::db::get_pool();
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    conn.exec_drop(
        "INSERT INTO mc_fleet_status (agent_name, tailscale_ip, status, gateway_port, gateway_token, os_info) VALUES (?, ?, 'offline', ?, ?, ?) ON DUPLICATE KEY UPDATE tailscale_ip=VALUES(tailscale_ip), gateway_port=VALUES(gateway_port), gateway_token=VALUES(gateway_token), os_info=VALUES(os_info)",
        (&agent_name, host, port, &token, &os_name),
    ).await?;
    println!("✅");

    // Step 8: Restart gateway and verify
    print!("  [8/8] Restarting gateway... ");
    let restart_cmd = format!("{}openclaw gateway restart 2>&1 | tail -1", pfx);
    let _ = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("ssh").args(ssh_args(&restart_cmd)).output(),
    )
    .await;

    // Verify via HTTP API
    tokio::time::sleep(Duration::from_secs(3)).await;
    let url = format!("http://{}:{}/v1/chat/completions", host, port);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let body = serde_json::json!({
        "model": "openclaw:main",
        "messages": [{"role": "user", "content": "Say 'online' in one word."}]
    });
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                println!("✅ Gateway responding!");
                // Update status
                conn.exec_drop(
                    "UPDATE mc_fleet_status SET status='online' WHERE agent_name=?",
                    (&agent_name,),
                )
                .await?;
            } else {
                println!("⚠️  Gateway returned {}", resp.status());
            }
        }
        Err(_) => println!("⚠️  Gateway not responding yet (may need manual restart on macOS)"),
    }

    pool.disconnect().await?;

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ✅ Agent '{}' onboarded at {}", agent_name, host);
    println!("     Port: {} | Token: {}...", port, &token[..12]);
    println!("     Run `sam` to see it in the fleet.\n");

    Ok(())
}

fn rand_byte() -> u8 {
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ((t.subsec_nanos() ^ (t.as_secs() as u32).wrapping_mul(2654435761)) & 0xFF) as u8
}

/// Deploy workspace file to agent(s)
pub async fn run_deploy(
    target: &str,
    file: &str,
    source: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;

    // Resolve source file
    let src_path = source
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("templates/{}", file));

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

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=3",
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "BatchMode=yes",
                    &format!("admin@{}", ip),
                    workspace_cmd,
                ])
                .output(),
        )
        .await;

        let workspace = match out {
            Ok(Ok(o)) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => {
                println!("❌ unreachable");
                continue;
            }
        };

        // SCP the file
        let dest = format!("admin@{}:{}/{}", ip, workspace, file);
        let scp_out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Command::new("scp")
                .args([
                    "-o",
                    "ConnectTimeout=3",
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "BatchMode=yes",
                    &src_path,
                    &dest,
                ])
                .output(),
        )
        .await;

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
pub async fn run_init(
    db_host: Option<&str>,
    db_port: Option<u16>,
    db_user: Option<&str>,
    db_pass: Option<&str>,
    db_name: Option<&str>,
    self_ip: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use mysql_async::prelude::*;
    use std::io::{self, Write};

    println!("\n   {} {}\n", c_bold_cyan("🩺"), c_bold("Fleet Doctor"));
    print_divider();
    println!();

    let prompt = |label: &str, default: &str| -> String {
        print!("  {} [{}]: ", label, default);
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let v = input.trim().to_string();
        if v.is_empty() { default.to_string() } else { v }
    };

    // Interactive prompts for missing values
    let db_host = db_host
        .map(|s| s.to_string())
        .unwrap_or_else(|| prompt("MySQL host", "127.0.0.1"));
    let db_port = db_port.unwrap_or_else(|| prompt("MySQL port", "3306").parse().unwrap_or(3306));
    let db_user = db_user
        .map(|s| s.to_string())
        .unwrap_or_else(|| prompt("MySQL user", "root"));
    let db_pass = db_pass.map(|s| s.to_string()).unwrap_or_else(|| {
        print!("  MySQL password: ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        input.trim().to_string()
    });
    let db_name = db_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| prompt("Database name", "sam_fleet"));
    let self_ip = self_ip.map(|s| s.to_string()).unwrap_or_else(|| {
        let detected = std::process::Command::new("hostname")
            .arg("-I")
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .next()
                    .unwrap_or("127.0.0.1")
                    .to_string()
            })
            .unwrap_or_else(|_| "127.0.0.1".into());
        prompt("This machine's IP", &detected)
    });

    println!();

    // Step 1: Connect to DB
    print!("  [1/4] Connecting to MySQL... ");
    let encoded_pass = db_pass.replace("$", "%24").replace("@", "%40");
    let url = format!(
        "mysql://{}:{}@{}:{}/{}",
        db_user, encoded_pass, db_host, db_port, db_name
    );
    let pool = mysql_async::Pool::new(url.as_str());
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| format!("DB connection failed: {}", e))?;
    println!("✅");

    // Step 2: Create tables
    print!("  [2/4] Creating tables... ");
    conn.query_drop(
        r"
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
    ",
    )
    .await?;
    conn.query_drop(
        r"
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
    ",
    )
    .await?;
    conn.query_drop(
        r"
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
    ",
    )
    .await?;
    println!("✅ mc_fleet_status, mc_chat, mc_task_routing");

    // Step 3: Generate config
    print!("  [3/4] Generating config... ");
    let cfg = SamConfig {
        database: DatabaseConfig {
            url: Some(url.clone()),
            host: Some(db_host.clone()),
            port: Some(db_port),
            user: Some(db_user.clone()),
            password: Some(db_pass.clone()),
            database: Some(db_name.clone()),
        },
        tui: TuiConfig::default(),
        fleet: FleetConfig::default(),
        identity: IdentityConfig {
            user: whoami().unwrap_or_else(|| "operator".into()),
        },
    };
    cfg.save()?;

    // Also write .env
    let env_path = std::path::Path::new(".env");
    if !env_path.exists() {
        std::fs::write(
            env_path,
            format!(
                "SAM_DB_URL={}\nSAM_SELF_IP={}\nSAM_USER={}\n",
                url, self_ip, cfg.identity.user,
            ),
        )?;
    }
    println!("✅ ~/.config/sam/config.toml");

    // Step 4: Self-register
    print!("  [4/4] Registering this machine... ");
    let hostname = std::process::Command::new("hostname")
        .output()
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
    std::process::Command::new("whoami")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Diagnose and auto-fix fleet issues
pub async fn run_doctor(
    fix: bool,
    agent_filter: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;

    print_banner();

    let pool = crate::db::get_pool();
    let agents = crate::db::load_fleet(&pool).await?;

    let targets: Vec<&crate::db::DbAgent> = if let Some(name) = agent_filter {
        agents
            .iter()
            .filter(|a| a.agent_name.contains(name))
            .collect()
    } else {
        agents.iter().collect()
    };

    let mut issues = 0;
    let mut fixed = 0;

    for agent in &targets {
        let name = &agent.agent_name;
        let ip = agent.tailscale_ip.as_deref().unwrap_or("?");
        let user = "admin";
        let is_mac = agent
            .os_info
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("mac");
        let pfx = if is_mac {
            "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
        } else {
            ""
        };

        println!("  🔍 {} ({})", name, ip);

        // Check 1: SSH connectivity
        let ssh_ok = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=3",
                    "-o",
                    "BatchMode=yes",
                    &format!("{}@{}", user, ip),
                    "echo ok",
                ])
                .output(),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

        if !ssh_ok {
            println!("     ❌ SSH unreachable — skipping");
            issues += 1;
            continue;
        }
        println!("     ✅ SSH");

        // Check 2: OpenClaw installed
        let oc_out = Command::new("ssh")
            .args([
                "-o",
                "ConnectTimeout=3",
                "-o",
                "BatchMode=yes",
                &format!("{}@{}", user, ip),
                &format!(
                    "{}openclaw --version 2>/dev/null || echo NOT_INSTALLED",
                    pfx
                ),
            ])
            .output()
            .await
            .ok();
        let oc_version = oc_out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if oc_version.contains("NOT_INSTALLED") || oc_version.is_empty() {
            println!("     ❌ OpenClaw not installed");
            issues += 1;
            if fix {
                print!("        🔧 Installing... ");
                let cmd = if is_mac {
                    format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx)
                } else {
                    "sudo npm install -g openclaw@latest 2>&1 | tail -1".into()
                };
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=3",
                            "-o",
                            "BatchMode=yes",
                            &format!("{}@{}", user, ip),
                            &cmd,
                        ])
                        .output(),
                )
                .await;
                println!("done");
                fixed += 1;
            }
        } else {
            println!("     ✅ OpenClaw {}", oc_version);
        }

        // Check 3: Gateway running
        let gw_out = Command::new("ssh")
            .args([
                "-o",
                "ConnectTimeout=3",
                "-o",
                "BatchMode=yes",
                &format!("{}@{}", user, ip),
                "ss -tlnp 2>/dev/null | grep 18789 | head -1",
            ])
            .output()
            .await
            .ok();
        let gw_line = gw_out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if gw_line.is_empty() {
            println!("     ❌ Gateway not running");
            issues += 1;
            if fix {
                print!("        🔧 Starting gateway... ");
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=3",
                            "-o",
                            "BatchMode=yes",
                            &format!("{}@{}", user, ip),
                            &format!("{}openclaw gateway restart 2>&1 | tail -1", pfx),
                        ])
                        .output(),
                )
                .await;
                println!("done");
                fixed += 1;
            }
        } else {
            let binding = if gw_line.contains("0.0.0.0") {
                "0.0.0.0 ✅"
            } else {
                "localhost ⚠️"
            };
            println!("     ✅ Gateway running ({})", binding);

            // Check 3b: Binding
            if !gw_line.contains("0.0.0.0") {
                println!("     ❌ Gateway bound to localhost (needs bind=lan)");
                issues += 1;
                if fix {
                    print!("        🔧 Setting bind=lan... ");
                    let _ = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "BatchMode=yes",
                        &format!("{}@{}", user, ip),
                        "python3 -c \"import json,os;p=os.path.expanduser('~/.openclaw/openclaw.json');c=json.load(open(p));c.setdefault('gateway',{})['bind']='lan';json.dump(c,open(p,'w'),indent=2);print('ok')\""
                    ]).output().await;
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        Command::new("ssh")
                            .args([
                                "-o",
                                "ConnectTimeout=3",
                                "-o",
                                "BatchMode=yes",
                                &format!("{}@{}", user, ip),
                                &format!("{}openclaw gateway restart 2>&1 | tail -1", pfx),
                            ])
                            .output(),
                    )
                    .await;
                    println!("done");
                    fixed += 1;
                }
            }
        }

        // Check 4: chatCompletions enabled
        let cc_out = Command::new("ssh").args([
            "-o", "ConnectTimeout=3", "-o", "BatchMode=yes",
            &format!("{}@{}", user, ip),
            "python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{}).get('http',{}).get('endpoints',{}).get('chatCompletions',{}).get('enabled',False))\""
        ]).output().await.ok();
        let cc_enabled = cc_out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if cc_enabled != "True" {
            println!("     ❌ chatCompletions not enabled");
            issues += 1;
            if fix {
                print!("        🔧 Enabling chatCompletions... ");
                let _ = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "BatchMode=yes",
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
                let tok_out = Command::new("ssh").args(["-o", "ConnectTimeout=3", "-o", "BatchMode=yes",
                    &format!("{}@{}", user, ip),
                    "python3 -c \"import json,os;c=json.load(open(os.path.expanduser('~/.openclaw/openclaw.json')));print(c.get('gateway',{}).get('auth',{}).get('token',''))\""
                ]).output().await.ok();
                let token = tok_out
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                if !token.is_empty() {
                    let mut conn = pool.get_conn().await?;
                    use mysql_async::prelude::*;
                    conn.exec_drop(
                        "UPDATE mc_fleet_status SET gateway_token=? WHERE agent_name=?",
                        (&token, name),
                    )
                    .await?;
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
            .build()
            .unwrap_or_default()
            .get(format!("http://{}:18789/", ip))
            .send()
            .await
            .is_ok();
        if http_ok {
            println!("     ✅ HTTP API reachable");
        } else {
            println!("     ⚠️  HTTP API unreachable (SSH fallback will be used)");
        }

        println!();
    }

    pool.disconnect().await?;

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "  {} agents checked, {} issues found, {} fixed",
        targets.len(),
        issues,
        fixed
    );
    if issues > 0 && !fix {
        println!("  Run `sam doctor --fix` to auto-repair");
    }
    println!();

    Ok(())
}
