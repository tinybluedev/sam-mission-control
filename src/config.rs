//! Configuration loading for S.A.M Mission Control.
//!
//! This module handles two configuration files:
//!
//! - **`config.toml`** — database credentials, TUI settings, operator identity.
//!   Loaded by the CLI layer from `~/.config/sam/config.toml`.
//! - **`fleet.toml`** — list of agents with display names, emojis, locations, and
//!   SSH usernames. Loaded via [`load_fleet_config`].
//!
//! Agent name resolution (display name / alias → canonical name) is provided by
//! [`resolve_alias`].

use serde::Deserialize;
use std::path::PathBuf;

/// Top-level fleet configuration, parsed from `fleet.toml`.
#[derive(Debug, Deserialize)]
pub struct FleetConfig {
    pub agent: Vec<AgentConfig>,
}

/// Configuration for a single agent entry in `fleet.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub display: Option<String>,
    pub emoji: Option<String>,
    pub location: Option<String>,
    pub ssh_user: Option<String>,
    pub host: Option<String>,  // override Tailscale IP with LAN IP or hostname
    pub jump_host: Option<String>,
    pub jump_user: Option<String>,
}

impl AgentConfig {
    /// Return the display name, falling back to the agent's `name` field.
    pub fn display_name(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }
    /// Return the emoji prefix, defaulting to `"❓"`.
    pub fn emoji(&self) -> &str {
        self.emoji.as_deref().unwrap_or("❓")
    }
    /// Return the location label, defaulting to `"Unknown"`.
    pub fn location(&self) -> &str {
        self.location.as_deref().unwrap_or("Unknown")
    }
    /// Return the SSH username, defaulting to `"root"`.
    pub fn ssh_user(&self) -> &str {
        self.ssh_user.as_deref().unwrap_or("root")
    }
    /// Return the SSH jump host, if configured.
    pub fn jump_host(&self) -> Option<&str> {
        self.jump_host.as_deref().filter(|s| !s.trim().is_empty())
    }
    /// Return the SSH jump username, defaulting to the agent SSH user when omitted.
    pub fn jump_user(&self) -> Option<&str> {
        self.jump_host()
            .map(|_| self.jump_user.as_deref().unwrap_or(self.ssh_user()))
    }
}

/// Find and load fleet.toml from:
/// 1. $SAM_FLEET_CONFIG env var
/// 2. ./fleet.toml (current directory)
/// 3. ~/.config/sam/fleet.toml
pub fn fleet_config_path() -> PathBuf {
    let paths: Vec<PathBuf> = vec![
        std::env::var("SAM_FLEET_CONFIG").ok().map(PathBuf::from),
        Some(PathBuf::from("fleet.toml")),
        dirs_next().map(|d| d.join("fleet.toml")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for path in &paths {
        if path.exists() {
            return path.clone();
        }
    }
    dirs_next().map(|d| d.join("fleet.toml")).unwrap_or_else(|| PathBuf::from("fleet.toml"))
}

pub fn load_fleet_config() -> Result<FleetConfig, String> {
    let paths: Vec<PathBuf> = vec![
        std::env::var("SAM_FLEET_CONFIG").ok().map(PathBuf::from),
        Some(PathBuf::from("fleet.toml")),
        dirs_next().map(|d| d.join("fleet.toml")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in &paths {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
            let config: FleetConfig = toml::from_str(&content)
                .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
            return Ok(config);
        }
    }

    Err(format!(
        "No fleet.toml found. Searched: {:?}\nCopy fleet.example.toml to fleet.toml and configure your fleet.",
        paths
    ))
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config").join("sam"))
}

/// Resolve chat target aliases. Returns the canonical agent name.
#[allow(dead_code)]
pub fn resolve_alias(input: &str, agents: &[AgentConfig]) -> String {
    let lower = input.to_lowercase();
    // Direct name match
    for a in agents {
        if a.name.to_lowercase() == lower {
            return a.name.clone();
        }
        if let Some(d) = &a.display
            && d.to_lowercase() == lower
        {
            return a.name.clone();
        }
    }
    // Partial match (starts with)
    for a in agents {
        if a.name.to_lowercase().starts_with(&lower) {
            return a.name.clone();
        }
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::{AgentConfig, resolve_alias};

    fn agent(name: &str, display: Option<&str>) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            display: display.map(ToString::to_string),
            emoji: None,
            location: None,
            ssh_user: None,
            host: None,
            jump_host: None,
            jump_user: None,
        }
    }

    #[test]
    fn resolve_alias_prefers_exact_name_and_display() {
        let agents = vec![
            agent("webserver", Some("Web Server")),
            agent("gpu-node", Some("GPU Node")),
        ];
        assert_eq!(resolve_alias("webserver", &agents), "webserver");
        assert_eq!(resolve_alias("web server", &agents), "webserver");
    }

    #[test]
    fn resolve_alias_supports_prefix_and_unknown_fallback() {
        let agents = vec![agent("gpu-node", Some("GPU Node"))];
        assert_eq!(resolve_alias("gpu", &agents), "gpu-node");
        assert_eq!(resolve_alias("UNLISTED", &agents), "unlisted");
    }

    #[test]
    fn jump_user_defaults_to_agent_ssh_user_when_jump_host_is_set() {
        let cfg = AgentConfig {
            name: "agent-a".into(),
            display: None,
            emoji: None,
            location: None,
            ssh_user: Some("ubuntu".into()),
            host: None,
            jump_host: Some("bastion.internal".into()),
            jump_user: None,
        };
        assert_eq!(cfg.jump_host(), Some("bastion.internal"));
        assert_eq!(cfg.jump_user(), Some("ubuntu"));
    }

    #[test]
    fn jump_user_is_none_without_jump_host() {
        let cfg = AgentConfig {
            name: "agent-a".into(),
            display: None,
            emoji: None,
            location: None,
            ssh_user: Some("ubuntu".into()),
            host: None,
            jump_host: None,
            jump_user: Some("bastion-user".into()),
        };
        assert_eq!(cfg.jump_host(), None);
        assert_eq!(cfg.jump_user(), None);
    }
}
