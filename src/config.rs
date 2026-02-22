use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct FleetConfig {
    pub agent: Vec<AgentConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub display: Option<String>,
    pub emoji: Option<String>,
    pub location: Option<String>,
    pub ssh_user: Option<String>,
}

impl AgentConfig {
    pub fn display_name(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }
    pub fn emoji(&self) -> &str {
        self.emoji.as_deref().unwrap_or("❓")
    }
    pub fn location(&self) -> &str {
        self.location.as_deref().unwrap_or("Unknown")
    }
    pub fn ssh_user(&self) -> &str {
        self.ssh_user.as_deref().unwrap_or("root")
    }
}

/// Find and load fleet.toml from:
/// 1. $SAM_FLEET_CONFIG env var
/// 2. ./fleet.toml (current directory)
/// 3. ~/.config/sam/fleet.toml
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
}
