//! S.A.M Mission Control — main entry point.
//!
//! Parses CLI arguments, loads configuration, and either runs a CLI subcommand
//! (non-interactive) or launches the full Ratatui TUI event loop.

mod cli;
mod config;
mod wizard;
mod db;
mod theme;
mod validate;
mod shell;

use clap::Parser;
use dotenvy;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, MouseEventKind, MouseButton},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    prelude::*,
    widgets::*,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::stdout;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::mpsc;

use theme::{BgDensity, Theme, ThemeName};

// ---- Data ----

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Agent {
    name: String,
    db_name: String,
    emoji: String,
    host: String,
    location: String,
    status: AgentStatus,
    os: String,
    kernel: String,
    oc_version: String,
    last_seen: String,
    current_task: Option<String>,
    ssh_user: String,
    capabilities: Vec<String>,
    token_burn: i32,
    latency_ms: Option<u32>,
    cpu_pct: Option<f32>,
    ram_pct: Option<f32>,
    disk_pct: Option<f32>,
    gateway_port: i32,
    gateway_token: Option<String>,
    uptime_seconds: i64,
    activity: String,  // What the agent is currently doing
    context_pct: Option<f32>,  // Context window usage %
    #[serde(skip)]
    last_probe_at: Option<std::time::Instant>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum AgentStatus { Online, Busy, Offline, Probing, Unknown }

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Online  => write!(f, "●  online"),
            Self::Busy    => write!(f, "◉  busy"),
            Self::Offline => write!(f, "○  offline"),
            Self::Probing => write!(f, "⟳  probing"),
            Self::Unknown => write!(f, "?  unknown"),
        }
    }
}

impl AgentStatus {
    fn from_str(s: &str) -> Self {
        match s { "online" => Self::Online, "busy" => Self::Busy, "offline"|"error" => Self::Offline, _ => Self::Unknown }
    }
    fn to_db_str(&self) -> &str {
        match self { Self::Online => "online", Self::Busy => "busy", _ => "offline" }
    }
}

#[derive(Clone, Debug)]
struct Alert {
    time: String,
    agent: String,
    emoji: String,
    message: String,
    severity: AlertSeverity,
}

#[derive(Clone, Debug, PartialEq)]
enum AlertSeverity { Critical, Warning, Info }

impl AlertSeverity {
    fn icon(&self) -> &str {
        match self { Self::Critical => "🔴", Self::Warning => "🟡", Self::Info => "🔵" }
    }
}

#[derive(Clone, Debug)]
struct ChatLine {
    sender: String,
    target: Option<String>,
    message: String,
    response: Option<String>,
    time: String,
    status: String,
    kind: String,
}

#[derive(PartialEq, Clone)]
enum Focus { Fleet, Chat, AgentChat, Command, Workspace, Services }

#[derive(PartialEq)]
enum Screen { Dashboard, AgentDetail, TaskBoard, SpawnManager, VpnStatus, Alerts, Help }

#[derive(PartialEq, Clone, Copy)]
enum SortMode { Name, Status, Location, Version, Latency }

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Name => Self::Status,
            Self::Status => Self::Location,
            Self::Location => Self::Version,
            Self::Version => Self::Latency,
            Self::Latency => Self::Name,
        }
    }
    fn label(&self) -> &str {
        match self {
            Self::Name => "name", Self::Status => "status",
            Self::Location => "location", Self::Version => "version",
            Self::Latency => "latency",
        }
    }
    fn arrow(&self) -> &str { "▲" }
}

struct ProbeResult {
    index: usize,
    status: AgentStatus,
    os: String,
    kernel: String,
    oc_version: String,
    latency_ms: Option<u32>,
    cpu_pct: Option<f32>,
    ram_pct: Option<f32>,
    disk_pct: Option<f32>,
    activity: String,
    context_pct: Option<f32>,
}


// ── UI Helpers ──────────────────────────────────────
fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let secs = now % 86400;
    let hours = ((secs / 3600) + 24 - 6) % 24; // UTC-6 for CST
    let mins = (secs % 3600) / 60;
    format!("{:02}:{:02}", hours, mins)
}

fn os_emoji(os: &str) -> &'static str {
    let os_lower = os.to_lowercase();
    if os_lower.contains("mac") || os_lower.contains("darwin") { "🍎" }
    else if os_lower.contains("windows") { "🪟" }
    else if os_lower.contains("android") { "📱" }
    else if os_lower.contains("arch") { "🏔" }
    else if os_lower.contains("fedora") { "🎩" }
    else if os_lower.contains("ubuntu") { "🟠" }
    else if os_lower.contains("rhel") || os_lower.contains("alma") || os_lower.contains("rocky") { "🔴" }
    else if os_lower.contains("linux") { "🐧" }
    else { "💻" }
}

fn format_uptime(secs: i64) -> String {
    if secs <= 0 { return "—".into(); }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 { format!("{}d {}h", days, hours) }
    else if hours > 0 { format!("{}h {}m", hours, mins) }
    else { format!("{}m", mins) }
}

fn format_last_seen(dt: &str) -> String {
    // Simple relative time from datetime string
    if dt.is_empty() { return "—".into(); }
    // Just show the time portion for now
    if let Some(time) = dt.split(' ').nth(1) {
        time[..5].to_string()  // HH:MM
    } else {
        dt.to_string()
    }
}

fn format_since(instant: Option<std::time::Instant>) -> String {
    match instant {
        None => "—".into(),
        Some(i) => {
            let secs = i.elapsed().as_secs();
            if secs < 60 { format!("{}s", secs) }
            else if secs < 3600 { format!("{}m", secs / 60) }
            else { format!("{}h", secs / 3600) }
        }
    }
}

fn last_seen_color(instant: Option<std::time::Instant>, t: &Theme) -> Color {
    match instant {
        None => t.text_dim,
        Some(i) => {
            let secs = i.elapsed().as_secs();
            if secs < 300 { t.status_online }
            else if secs < 1800 { t.status_busy }
            else { t.status_offline }
        }
    }
}

fn ping_color(latency: Option<u32>) -> ratatui::style::Color {
    match latency {
        Some(ms) if ms < 100 => ratatui::style::Color::Green,
        Some(ms) if ms < 500 => ratatui::style::Color::Yellow,
        Some(_) => ratatui::style::Color::Red,
        None => ratatui::style::Color::DarkGray,
    }
}

fn resource_bar(pct: Option<f32>, width: u16) -> String {
    let p = pct.unwrap_or(0.0);
    let filled = ((p / 100.0) * width as f32) as usize;
    let empty = (width as usize).saturating_sub(filled);
    format!("{}{}",
        "█".repeat(filled),
        "░".repeat(empty),
    )
}


struct App {
    agents: Vec<Agent>,
    fleet_config: Vec<config::AgentConfig>,
    selected: usize,
    screen: Screen,
    focus: Focus,
    should_quit: bool,
    last_refresh: Instant,
    last_chat_poll: Instant,
    status_message: String,
    toast_message: Option<String>,
    toast_at: Option<Instant>,
    db_pool: Option<mysql_async::Pool>,
    chat_input: String,
    chat_history: Vec<ChatLine>,
    chat_scroll: u16,
    agent_chat_input: String,
    agent_chat_history: Vec<ChatLine>,  // Direct messages to focused agent
    agent_chat_scroll: u16,
    refresh_rx: Option<mpsc::UnboundedReceiver<ProbeResult>>,
    refreshing: bool,
    self_ip: String,
    // Fleet command
    command_input: String,
    // Wizard
    wizard: wizard::AgentWizard,
    // Task board
    tasks: Vec<db::Task>,
    task_filter_agent: Option<String>,
    task_selected: usize,
    task_input: String,
    task_input_active: bool,
    last_task_poll: Instant,
    // UI state
    spinner_frame: usize,
    sort_mode: SortMode,
    // Layout hit zones (updated each frame)
    fleet_area: Rect,
    chat_area: Rect,
    detail_info_area: Rect,
    detail_chat_area: Rect,
    fleet_row_start_y: u16,  // Y offset where first agent row starts
    // Splash
    spawned_agents: Vec<db::SpawnedAgent>,
    show_splash: bool,
    splash_start: Instant,
    // Alerts
    alerts: Vec<Alert>,
    alert_flash: Option<Instant>,
    gateway_confirm_at: Option<Instant>,
    // Diagnostics (inline doctor/fix)
    diag_active: bool,
    diag_steps: Vec<DiagStep>,
    diag_rx: Option<mpsc::UnboundedReceiver<DiagStep>>,
    diag_auto_fix: bool,
    // Services (OpenClaw plugin management)
    svc_list: Vec<ServiceEntry>,
    svc_selected: usize,
    svc_config: Option<serde_json::Value>,  // Full openclaw.json
    svc_loading: bool,
    svc_load_rx: Option<mpsc::UnboundedReceiver<Option<serde_json::Value>>>,
    config_load_rx: Option<mpsc::UnboundedReceiver<Option<String>>>,
    svc_detail_scroll: u16,
    // Workspace (agent file management)
    ws_files: Vec<WorkspaceFile>,
    ws_selected: usize,
    ws_content: Option<String>,
    ws_content_scroll: u16,
    ws_editing: bool,
    ws_edit_buffer: String,
    ws_crons: Vec<CronEntry>,
    ws_loading: bool,
    ws_load_rx: Option<mpsc::UnboundedReceiver<(Vec<WorkspaceFile>, Vec<CronEntry>)>>,
    ws_file_rx: Option<mpsc::UnboundedReceiver<String>>,
    // Filter
    filter_active: bool,
    filter_text: String,
    // Config viewer
    config_text: Option<String>,
    config_scroll: u16,
    // Multi-select
    multi_selected: std::collections::HashSet<usize>,
    // Theme
    theme_name: ThemeName,
    bg_density: BgDensity,
    theme: Theme,
    // Routing
    routed_msg_ids: std::collections::HashSet<i64>,
    // Background chat poll
    chat_poll_rx: Option<mpsc::UnboundedReceiver<ChatPollResult>>,
    chat_polling: bool,
    // Autocomplete
    ac_visible: bool,
    ac_matches: Vec<String>,
    ac_selected: usize,
    ac_start_pos: usize,  // cursor position of the '@'
}

impl App {
    async fn new(fleet_config: config::FleetConfig) -> Self {
        let pool = db::get_pool();
        let self_ip = std::env::var("SAM_SELF_IP").unwrap_or_else(|_| "localhost".into());
        let mut agents = Vec::new();

        match db::load_fleet(&pool).await {
            Ok(db_agents) => {
                for da in db_agents {
                    let cfg = fleet_config.agent.iter().find(|c| c.name == da.agent_name);
                    let caps: Vec<String> = da.capabilities.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default();
                    agents.push(Agent {
                        name: cfg.map(|c| c.display_name().to_string()).unwrap_or_else(|| da.agent_name.clone()),
                        db_name: da.agent_name.clone(),
                        emoji: cfg.map(|c| c.emoji().to_string()).unwrap_or_else(|| os_emoji(da.os_info.as_deref().unwrap_or("")).to_string()),
                        host: da.tailscale_ip.unwrap_or("?".into()),
                        location: cfg.map(|c| c.location().to_string()).unwrap_or_else(|| "?".into()),
                        status: AgentStatus::from_str(&da.status),
                        os: da.os_info.unwrap_or_default(),
                        kernel: da.kernel.unwrap_or_default(),
                        oc_version: da.oc_version.unwrap_or_default(),
                        last_seen: String::new(),
                        current_task: None,
                        ssh_user: cfg.map(|c| c.ssh_user().to_string()).unwrap_or_else(|| "root".into()),
                        capabilities: caps,
                        token_burn: da.token_burn_today,
                        latency_ms: None,
                        cpu_pct: None, ram_pct: None, disk_pct: None,
                        gateway_port: da.gateway_port,
                        gateway_token: da.gateway_token.clone(),
                        uptime_seconds: da.uptime_seconds,
                        activity: "idle".into(), context_pct: None,
                        last_probe_at: None,
                    });
                }
            }
            Err(e) => eprintln!("DB: {}", e),
        }

        let chat_history = match db::load_global_chat(&pool, 100).await {
            Ok(msgs) => msgs.iter().map(|m| ChatLine {
                sender: m.sender.clone(), target: m.target.clone(),
                message: m.message.clone(), response: m.response.clone(),
                time: m.created_at.clone(), status: m.status.clone(),
                kind: m.kind.clone(),
            }).collect(),
            Err(_) => vec![],
        };

        let tn = ThemeName::Standard;
        let bd = BgDensity::Dark;

        App {
            fleet_config: fleet_config.agent,
            agents, selected: 0, screen: Screen::Dashboard, focus: Focus::Fleet,
            should_quit: false, last_refresh: Instant::now(), last_chat_poll: Instant::now(),
            status_message: String::new(), toast_message: None, toast_at: None,
            db_pool: Some(pool),
            chat_input: String::new(), chat_history, chat_scroll: 0,
            agent_chat_input: String::new(), agent_chat_history: vec![], agent_chat_scroll: 0,
            refresh_rx: None, refreshing: false, self_ip,
            command_input: String::new(),
            wizard: wizard::AgentWizard::new(),
            tasks: vec![], task_filter_agent: None, task_selected: 0, task_input: String::new(), task_input_active: false,
            last_task_poll: Instant::now(),
            spawned_agents: vec![], show_splash: true, splash_start: Instant::now(),
            config_text: None, config_scroll: 0,
            filter_active: false, filter_text: String::new(),
            alerts: vec![], alert_flash: None, gateway_confirm_at: None,
            multi_selected: HashSet::new(),
            spinner_frame: 0, sort_mode: SortMode::Name,
            fleet_area: Rect::default(), chat_area: Rect::default(),
            detail_info_area: Rect::default(), detail_chat_area: Rect::default(),
            fleet_row_start_y: 0,
            theme_name: tn, bg_density: bd, theme: Theme::resolve(tn, bd), routed_msg_ids: std::collections::HashSet::new(),
            diag_active: false, diag_steps: vec![], diag_rx: None, diag_auto_fix: false,
            svc_list: vec![], config_load_rx: None, svc_selected: 0, svc_config: None, svc_loading: false, svc_load_rx: None, svc_detail_scroll: 0,
            ws_files: vec![], ws_selected: 0, ws_content: None, ws_content_scroll: 0, ws_load_rx: None, ws_file_rx: None,
            ws_editing: false, ws_edit_buffer: String::new(), ws_crons: vec![], ws_loading: false, chat_poll_rx: None, chat_polling: false, ac_visible: false, ac_matches: vec![], ac_selected: 0, ac_start_pos: 0,
        }
    }

    fn next(&mut self) {
        if self.agents.is_empty() { return; }
        self.selected = (self.selected + 1) % self.agents.len();
    }
    fn previous(&mut self) {
        if self.agents.is_empty() { return; }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.agents.len() - 1);
    }

    fn toast(&mut self, msg: &str) {
        self.toast_message = Some(msg.to_string());
        self.toast_at = Some(Instant::now());
    }

    fn user(&self) -> String { std::env::var("SAM_USER").unwrap_or_else(|_| "operator".into()) }

    /// Build a system prompt that gives agents awareness of the fleet and how to communicate
    fn build_system_prompt(&self, target_agent: Option<&str>) -> String {
        let agent_list: Vec<String> = self.agents.iter()
            .map(|a| {
                let status = format!("{}", a.status);
                format!("  - @{} ({}{})", a.db_name, a.location,
                    if status == "online" { "" } else { ", offline" })
            })
            .collect();

        let context = if let Some(target) = target_agent {
            format!("You are @{}. This is a direct message from the operator.", target)
        } else {
            "This is a broadcast message to all agents.".to_string()
        };

        format!(
            "You are an AI agent in S.A.M Mission Control — a fleet management TUI.
            {}

            ## Fleet Agents
{}

            ## Communication
            - To tag another agent in your response, use @agent-name (e.g. @nix, @cyber)
            - Tagged agents will automatically receive your message
            - Use this for delegation, questions, or coordination
            - Keep responses concise — this is a terminal UI with limited width
            - The operator\'s name is: {}
",
            context,
            agent_list.join("
"),
            self.user()
        )
    }

    fn cycle_theme(&mut self) {
        self.theme_name = self.theme_name.next();
        self.theme = Theme::resolve(self.theme_name, self.bg_density);
    }

    fn cycle_sort(&mut self) {
        self.sort_mode = self.sort_mode.next();
        let sm = self.sort_mode;
        self.agents.sort_by(|a, b| match sm {
            SortMode::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortMode::Status => {
                let rank = |s: &AgentStatus| match s {
                    AgentStatus::Online => 0, AgentStatus::Busy => 1,
                    AgentStatus::Unknown => 2, AgentStatus::Probing => 3, AgentStatus::Offline => 4,
                };
                rank(&a.status).cmp(&rank(&b.status))
            }
            SortMode::Location => a.location.cmp(&b.location),
            SortMode::Version => b.oc_version.cmp(&a.oc_version),
            SortMode::Latency => {
                let lat = |a: &Agent| a.latency_ms.unwrap_or(9999);
                lat(a).cmp(&lat(b))
            }
        });
    }

    fn cycle_bg(&mut self) {
        self.bg_density = self.bg_density.next();
        self.theme = Theme::resolve(self.theme_name, self.bg_density);
    }

    /// Get the active chat input (depending on screen)
    fn active_chat_input(&self) -> &str {
        if self.screen == Screen::AgentDetail { &self.agent_chat_input } else { &self.chat_input }
    }

    fn active_chat_input_mut(&mut self) -> &mut String {
        if self.screen == Screen::AgentDetail { &mut self.agent_chat_input } else { &mut self.chat_input }
    }

    /// Update autocomplete state based on current input
    fn update_autocomplete(&mut self) {
        let input = self.active_chat_input().to_string();
        // Find the last '@' that starts a mention
        if let Some(at_pos) = input.rfind('@') {
            let after_at = &input[at_pos + 1..];
            // Only trigger if we're still typing the mention (no space after)
            if !after_at.contains(' ') {
                let query = after_at.to_lowercase();
                let matches: Vec<String> = self.agents.iter()
                    .map(|a| a.db_name.clone())
                    .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
                    .collect();
                if !matches.is_empty() {
                    self.ac_visible = true;
                    self.ac_matches = matches;
                    self.ac_start_pos = at_pos;
                    if self.ac_selected >= self.ac_matches.len() {
                        self.ac_selected = 0;
                    }
                    return;
                }
            }
        }
        self.ac_visible = false;
        self.ac_matches.clear();
        self.ac_selected = 0;
    }

    /// Accept the currently selected autocomplete suggestion
    fn accept_autocomplete(&mut self) {
        if !self.ac_visible || self.ac_matches.is_empty() { return; }
        let name = self.ac_matches[self.ac_selected].clone();
        let pos = self.ac_start_pos;
        let replacement = format!("@{} ", name);
        if self.screen == Screen::AgentDetail {
            self.agent_chat_input.truncate(pos);
            self.agent_chat_input.push_str(&replacement);
        } else {
            self.chat_input.truncate(pos);
            self.chat_input.push_str(&replacement);
        }
        self.ac_visible = false;
        self.ac_matches.clear();
        self.ac_selected = 0;
    }

    fn agent_chat_lines(&self) -> &Vec<ChatLine> {
        &self.agent_chat_history
    }

    async fn send_message(&mut self) {
        if self.chat_input.trim().is_empty() { return; }
        let message = validate::sanitize_chat_message(&self.chat_input);
        self.chat_input.clear();
        if message.is_empty() { return; }

        // If message contains @mentions, only send to those agents. Otherwise broadcast to all.
        let mentioned: Vec<String> = {
            let mut m = Vec::new();
            for word in message.split_whitespace() {
                if let Some(name) = word.strip_prefix('@') {
                    let name_lower = name.to_lowercase();
                    if self.agents.iter().any(|a| a.db_name.to_lowercase() == name_lower) {
                        m.push(name_lower);
                    }
                }
            }
            m
        };
        let targeted = !mentioned.is_empty();
        let agent_names: Vec<String> = if targeted {
            self.agents.iter()
                .filter(|a| mentioned.contains(&a.db_name.to_lowercase()))
                .map(|a| a.db_name.clone())
                .collect()
        } else {
            self.agents.iter().map(|a| a.db_name.clone()).collect()
        };
        let display_target = if targeted {
            Some(agent_names.iter().map(|n| format!("@{}", n)).collect::<Vec<_>>().join(" "))
        } else { None };
        self.chat_history.push(ChatLine {
            sender: self.user(), target: display_target, message: message.clone(),
            response: None, time: now_str(), status: "pending".into(),
            kind: if targeted { "direct".into() } else { "global".into() },
        });

        if let Some(pool) = &self.db_pool {
            let ids = db::send_broadcast(pool, &self.user(), &message, &agent_names).await.unwrap_or_default();
            let sys_prompt = self.build_system_prompt(None);
            // Fire streaming AI requests to targeted agents (or all if broadcast)
            for (i, agent) in self.agents.iter().enumerate() {
                if targeted && !agent_names.contains(&agent.db_name) { continue; }
                if let Some(tok) = &agent.gateway_token {
                    let url = format!("http://{}:{}/v1/chat/completions", agent.host, agent.gateway_port);
                    let tok = tok.clone();
                    let msg = message.clone();
                    let pool = pool.clone();
                    let msg_id = ids.get(i).copied().unwrap_or(0);
                    let bcast_host = agent.host.clone();
                    let bcast_user = agent.ssh_user.clone();
                    let bcast_port = agent.gateway_port;
                    let sys_prompt = sys_prompt.clone();
                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(120))
                            .build().unwrap_or_default();
                        let _ = db::update_chat_status(&pool, msg_id, "connecting").await;
                        let body = serde_json::json!({
                            "model": "openclaw:main",
                            "stream": true,
                            "messages": [
                                {"role": "system", "content": sys_prompt},
                                {"role": "user", "content": msg}
                            ]
                        });
                        let result = client.post(&url)
                            .header("Authorization", format!("Bearer {}", tok))
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send().await;
                        match result {
                            Ok(resp) => {
                                use reqwest::header::CONTENT_TYPE;
                                let ct = resp.headers().get(CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                                if ct.contains("text/event-stream") || ct.contains("text/plain") {
                                    let _ = db::update_chat_status(&pool, msg_id, "thinking").await;
                                    use futures_util::StreamExt;
                                    let mut stream = resp.bytes_stream();
                                    let mut full = String::new();
                                    let mut last_write = std::time::Instant::now();
                                    let mut got = false;
                                    while let Some(chunk) = stream.next().await {
                                        let chunk = match chunk { Ok(c) => c, Err(_) => break };
                                        let text = String::from_utf8_lossy(&chunk);
                                        for line in text.lines() {
                                            let line = line.trim();
                                            if line == "data: [DONE]" || !line.starts_with("data: ") { continue; }
                                            if let Ok(j) = serde_json::from_str::<serde_json::Value>(&line[6..]) {
                                                if let Some(c) = j["choices"][0]["delta"]["content"].as_str() {
                                                    full.push_str(c); got = true;
                                                }
                                            }
                                        }
                                        if got && last_write.elapsed() > std::time::Duration::from_millis(300) {
                                            let _ = db::update_chat_partial(&pool, msg_id, &full).await;
                                            last_write = std::time::Instant::now();
                                        }
                                    }
                                    if full.is_empty() { full = "(empty response)".into(); }
                                    let _ = db::respond_to_chat(&pool, msg_id, &full).await;
                                } else {
                                    let _ = db::update_chat_status(&pool, msg_id, "thinking").await;
                                    match resp.json::<serde_json::Value>().await {
                                        Ok(j) => {
                                            let r = j["choices"][0]["message"]["content"]
                                                .as_str().unwrap_or("(no content)").to_string();
                                            let _ = db::respond_to_chat(&pool, msg_id, &r).await;
                                        }
                                        Err(e) => { let _ = db::respond_to_chat(&pool, msg_id, &format!("Parse error: {}", e)).await; }
                                    }
                                }
                            }
                            Err(_) => {
                                // SSH fallback (non-streaming)
                                let body_nostream = serde_json::json!({
                                    "model": "openclaw:main",
                                    "messages": [
                                        {"role": "system", "content": sys_prompt},
                                        {"role": "user", "content": msg}
                                    ]
                                });
                                let ssh_cmd = format!(
                                    "curl -sS --connect-timeout 10 -m 55 http://localhost:{}/v1/chat/completions -H 'Authorization: Bearer {}' -H 'Content-Type: application/json' -d {}",
                                    bcast_port, tok,
                                    shell::escape(&serde_json::to_string(&body_nostream).unwrap_or_default())
                                );
                                let response = match tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    tokio::process::Command::new("ssh")
                                        .args(["-o", "ConnectTimeout=2", "-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes",
                                            &format!("{}@{}", bcast_user, bcast_host), &ssh_cmd])
                                        .output()
                                ).await {
                                    Ok(Ok(o)) if o.status.success() => {
                                        let s = String::from_utf8_lossy(&o.stdout);
                                        serde_json::from_str::<serde_json::Value>(&s).ok()
                                            .and_then(|j| j["choices"][0]["message"]["content"].as_str().map(|s| s.to_string()))
                                            .unwrap_or_else(|| "⚠ SSH fallback parse error".into())
                                    }
                                    _ => "⚠ unreachable".into(),
                                };
                                let _ = db::respond_to_chat(&pool, msg_id, &response).await;
                            }
                        }
                    });
                }
            }
        }
        self.chat_scroll = 0;
    }

    async fn send_agent_message(&mut self) {
        if self.agent_chat_input.trim().is_empty() { return; }
        let message = validate::sanitize_chat_message(&self.agent_chat_input);
        self.agent_chat_input.clear();
        if message.is_empty() { return; }
        let agent = &self.agents[self.selected];
        let target = agent.db_name.clone();
        let host = agent.host.clone();
        let port = agent.gateway_port;
        let token = agent.gateway_token.clone();
        let ssh_user = agent.ssh_user.clone();

        self.agent_chat_history.push(ChatLine {
            sender: self.user(), target: Some(target.clone()), message: message.clone(),
            response: None, time: now_str(), status: "pending".into(),
            kind: "direct".into(),
        });

        // Store in DB
        let msg_id = if let Some(pool) = &self.db_pool {
            db::send_chat(pool, &self.user(), Some(&target), &message).await.unwrap_or(0)
        } else { 0 };

        // Fire AI request via OpenClaw HTTP API (streaming)
        if let Some(tok) = token {
            let pool = self.db_pool.clone();
            let sys_prompt = self.build_system_prompt(Some(&target));
            tokio::spawn(async move {
                let url = format!("http://{}:{}/v1/chat/completions", host, port);
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build().unwrap_or_default();

                // Mark as connecting
                if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "connecting").await; }

                let body = serde_json::json!({
                    "model": "openclaw:main",
                    "stream": true,
                    "messages": [
                        {"role": "system", "content": sys_prompt},
                        {"role": "user", "content": message}
                    ]
                });
                let result = client.post(&url)
                    .header("Authorization", format!("Bearer {}", tok))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send().await;

                match result {
                    Ok(resp) => {
                        use reqwest::header::CONTENT_TYPE;
                        let ct = resp.headers().get(CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok()).unwrap_or("").to_string();

                        if ct.contains("text/event-stream") || ct.contains("text/plain") {
                            // SSE streaming response
                            if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "thinking").await; }

                            use futures_util::StreamExt;
                            let mut stream = resp.bytes_stream();
                            let mut full_response = String::new();
                            let mut last_db_write = std::time::Instant::now();
                            let mut got_content = false;

                            while let Some(chunk) = stream.next().await {
                                let chunk = match chunk {
                                    Ok(c) => c,
                                    Err(_) => break,
                                };
                                let text = String::from_utf8_lossy(&chunk);
                                // Parse SSE lines: data: {"choices":[{"delta":{"content":"..."}}]}
                                for line in text.lines() {
                                    let line = line.trim();
                                    if line == "data: [DONE]" { continue; }
                                    if !line.starts_with("data: ") { continue; }
                                    let json_str = &line[6..];
                                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(json_str) {
                                        if let Some(content) = j["choices"][0]["delta"]["content"].as_str() {
                                            full_response.push_str(content);
                                            got_content = true;
                                        }
                                    }
                                }

                                // Write partial response to DB every 300ms
                                if got_content && last_db_write.elapsed() > std::time::Duration::from_millis(300) {
                                    if let Some(ref p) = pool {
                                        let _ = db::update_chat_partial(p, msg_id, &full_response).await;
                                    }
                                    last_db_write = std::time::Instant::now();
                                }
                            }

                            // Final write
                            if full_response.is_empty() { full_response = "(empty response)".into(); }
                            if let Some(ref p) = pool {
                                let _ = db::respond_to_chat(p, msg_id, &full_response).await;
                            }
                        } else {
                            // Non-streaming JSON response (fallback)
                            if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "thinking").await; }
                            match resp.json::<serde_json::Value>().await {
                                Ok(j) => {
                                    let response = j["choices"][0]["message"]["content"]
                                        .as_str().unwrap_or("(no content)").to_string();
                                    if let Some(ref p) = pool {
                                        let _ = db::respond_to_chat(p, msg_id, &response).await;
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref p) = pool {
                                        let _ = db::respond_to_chat(p, msg_id, &format!("Parse error: {}", e)).await;
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(ref p) = pool {
                            let _ = db::respond_to_chat(p, msg_id, &format!("Connection error: {}", e)).await;
                        }
                    }
                }
            });
        } else {
            if let Some(pool) = &self.db_pool {
                if msg_id > 0 {
                    let _ = db::respond_to_chat(pool, msg_id, "(no gateway token configured)").await;
                }
            }
        }
        self.agent_chat_scroll = 0;
    }

    /// Check agent responses for @mentions and route them as new messages
    async fn route_agent_mentions(&mut self, sender_agent: &str, response: &str) {
        let mut mentioned: Vec<String> = Vec::new();
        for word in response.split_whitespace() {
            if let Some(name) = word.strip_prefix('@') {
                // Clean trailing punctuation
                let clean = name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
                let clean_lower = clean.to_lowercase();
                if clean_lower != sender_agent.to_lowercase()
                    && self.agents.iter().any(|a| a.db_name.to_lowercase() == clean_lower)
                    && !mentioned.contains(&clean_lower)
                {
                    mentioned.push(clean_lower);
                }
            }
        }

        if mentioned.is_empty() { return; }

        // For each mentioned agent, forward the message
        for target_name in &mentioned {
            if let Some(agent) = self.agents.iter().find(|a| a.db_name.to_lowercase() == *target_name) {
                if let Some(tok) = &agent.gateway_token {
                    let pool = self.db_pool.clone();
                    let url = format!("http://{}:{}/v1/chat/completions", agent.host, agent.gateway_port);
                    let tok = tok.clone();
                    let from = sender_agent.to_string();
                    let msg = format!("[Message from @{}]: {}", sender_agent, response);
                    let sys = self.build_system_prompt(Some(&agent.db_name));
                    let target = agent.db_name.clone();

                    // Write to chat history
                    self.chat_history.push(ChatLine {
                        sender: from.clone(), target: Some(target.clone()),
                        message: format!("→ @{}", target), response: None,
                        time: now_str(), status: "routing".into(), kind: "direct".into(),
                    });

                    let msg_id = if let Some(ref p) = pool {
                        db::send_chat(p, &from, Some(&target), &format!("(routed from @{})", from)).await.unwrap_or(0)
                    } else { 0 };

                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(120))
                            .build().unwrap_or_default();
                        if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "connecting").await; }
                        let body = serde_json::json!({
                            "model": "openclaw:main",
                            "stream": true,
                            "messages": [
                                {"role": "system", "content": sys},
                                {"role": "user", "content": msg}
                            ]
                        });
                        let result = client.post(&url)
                            .header("Authorization", format!("Bearer {}", tok))
                            .header("Content-Type", "application/json")
                            .json(&body).send().await;
                        match result {
                            Ok(resp) => {
                                use reqwest::header::CONTENT_TYPE;
                                let ct = resp.headers().get(CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                                if ct.contains("text/event-stream") || ct.contains("text/plain") {
                                    if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "thinking").await; }
                                    use futures_util::StreamExt;
                                    let mut stream = resp.bytes_stream();
                                    let mut full = String::new();
                                    let mut last_write = std::time::Instant::now();
                                    let mut got = false;
                                    while let Some(chunk) = stream.next().await {
                                        let chunk = match chunk { Ok(c) => c, Err(_) => break };
                                        let text = String::from_utf8_lossy(&chunk);
                                        for line in text.lines() {
                                            let line = line.trim();
                                            if line == "data: [DONE]" || !line.starts_with("data: ") { continue; }
                                            if let Ok(j) = serde_json::from_str::<serde_json::Value>(&line[6..]) {
                                                if let Some(c) = j["choices"][0]["delta"]["content"].as_str() {
                                                    full.push_str(c); got = true;
                                                }
                                            }
                                        }
                                        if got && last_write.elapsed() > std::time::Duration::from_millis(300) {
                                            if let Some(ref p) = pool { let _ = db::update_chat_partial(p, msg_id, &full).await; }
                                            last_write = std::time::Instant::now();
                                        }
                                    }
                                    if full.is_empty() { full = "(empty response)".into(); }
                                    if let Some(ref p) = pool { let _ = db::respond_to_chat(p, msg_id, &full).await; }
                                } else {
                                    if let Some(ref p) = pool { let _ = db::update_chat_status(p, msg_id, "thinking").await; }
                                    match resp.json::<serde_json::Value>().await {
                                        Ok(j) => {
                                            let r = j["choices"][0]["message"]["content"].as_str().unwrap_or("(no content)").to_string();
                                            if let Some(ref p) = pool { let _ = db::respond_to_chat(p, msg_id, &r).await; }
                                        }
                                        Err(e) => { if let Some(ref p) = pool { let _ = db::respond_to_chat(p, msg_id, &format!("error: {}", e)).await; } }
                                    }
                                }
                            }
                            Err(e) => { if let Some(ref p) = pool { let _ = db::respond_to_chat(p, msg_id, &format!("unreachable: {}", e)).await; } }
                        }
                    });
                }
            }
        }
    }

    /// Load OpenClaw config from agent via SSH (non-blocking)
    fn start_services_load(&mut self) {
        if self.selected >= self.agents.len() { return; }
        if self.svc_loading { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        self.svc_loading = true;

        let (tx, rx) = mpsc::unbounded_channel();
        self.svc_load_rx = Some(rx);

        tokio::spawn(async move {
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), "cat ~/.openclaw/openclaw.json 2>/dev/null || echo null",
                ]).output()
            ).await;
            let config = match output {
                Ok(Ok(o)) if o.status.success() => {
                    let s = String::from_utf8_lossy(&o.stdout);
                    serde_json::from_str::<serde_json::Value>(s.trim()).ok()
                }
                _ => None,
            };
            let _ = tx.send(config);
        });
    }

    /// Parse services from loaded config
    fn parse_services(&mut self) {
        let mut services = Vec::new();
        if let Some(ref config) = self.svc_config {
            // Get enabled plugins
            let plugins = config.get("plugins").and_then(|p| p.get("entries"))
                .and_then(|e| e.as_object());
            let channels = config.get("channels").and_then(|c| c.as_object());

            // Collect all known services (from plugins + channels)
            let mut seen = std::collections::HashSet::new();
            if let Some(plugins) = plugins {
                for (name, val) in plugins {
                    seen.insert(name.clone());
                    let enabled = val.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
                    let has_channel = channels.map(|c| c.contains_key(name)).unwrap_or(false);
                    let summary = if has_channel {
                        self.build_channel_summary(name, config)
                    } else if enabled {
                        "enabled, no channel config".into()
                    } else {
                        "disabled".into()
                    };
                    services.push(ServiceEntry {
                        name: name.clone(), icon: svc_icon(name),
                        enabled, has_channel_config: has_channel, summary,
                    });
                }
            }
            // Also show channels that exist but aren't in plugins
            if let Some(channels) = channels {
                for name in channels.keys() {
                    if !seen.contains(name) {
                        seen.insert(name.clone());
                        let summary = self.build_channel_summary(name, config);
                        services.push(ServiceEntry {
                            name: name.clone(), icon: svc_icon(name),
                            enabled: false, has_channel_config: true,
                            summary: format!("no plugin entry — {}", summary),
                        });
                    }
                }
            }

            // Add gateway info
            if let Some(gw) = config.get("gateway") {
                let mode = gw.get("mode").and_then(|m| m.as_str()).unwrap_or("?");
                let has_token = gw.get("auth").and_then(|a| a.get("token")).is_some();
                let bind = gw.get("bind").and_then(|b| b.as_str()).unwrap_or("localhost");
                let chat = config.get("gateway").and_then(|g| g.get("chatCompletions"))
                    .and_then(|c| c.get("enabled")).and_then(|e| e.as_bool()).unwrap_or(false);
                services.insert(0, ServiceEntry {
                    name: "gateway".into(), icon: "🌐",
                    enabled: true, has_channel_config: false,
                    summary: format!("mode:{} bind:{} chat:{} auth:{}", mode, bind, if chat {"on"} else {"off"}, if has_token {"token"} else {"none"}),
                });
            }

            // Add model info
            if let Some(agents) = config.get("agents").and_then(|a| a.get("defaults")) {
                let model = agents.get("model").and_then(|m| m.get("primary"))
                    .and_then(|p| p.as_str()).unwrap_or("?");
                let ctx = agents.get("contextTokens").and_then(|c| c.as_u64()).unwrap_or(0);
                services.insert(0, ServiceEntry {
                    name: "model".into(), icon: "🧠",
                    enabled: true, has_channel_config: false,
                    summary: format!("{} ({}K ctx)", model.split('/').last().unwrap_or(model), ctx / 1000),
                });
            }
        }
        services.sort_by(|a, b| {
            // Gateway and model first, then enabled, then disabled
            let rank = |s: &ServiceEntry| -> u8 {
                if s.name == "model" { 0 }
                else if s.name == "gateway" { 1 }
                else if s.enabled { 2 }
                else { 3 }
            };
            rank(a).cmp(&rank(b))
        });
        self.svc_list = services;
    }

    fn build_channel_summary(&self, name: &str, config: &serde_json::Value) -> String {
        let ch = config.get("channels").and_then(|c| c.get(name));
        match ch {
            Some(v) => {
                let mut parts = Vec::new();
                if let Some(dm) = v.get("dmPolicy").and_then(|d| d.as_str()) {
                    parts.push(format!("dm:{}", dm));
                }
                if let Some(groups) = v.get("groups").and_then(|g| g.as_array()) {
                    parts.push(format!("{} groups", groups.len()));
                }
                if v.get("botToken").is_some() { parts.push("token:✓".into()); }
                if v.get("botId").is_some() { parts.push("botId:✓".into()); }
                if let Some(ch_arr) = v.get("channels").and_then(|c| c.as_array()) {
                    parts.push(format!("{} channels", ch_arr.len()));
                }
                if parts.is_empty() { "configured".into() } else { parts.join("  ") }
            }
            None => "no config".into(),
        }
    }

    /// Run diagnostics on focused agent (non-blocking, step-by-step)
    fn start_diagnostics(&mut self, fix: bool) {
        if self.selected >= self.agents.len() { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let name = agent.db_name.clone();
        let gw_port = agent.gateway_port;
        self.diag_active = true;
        self.diag_auto_fix = fix;
        self.diag_steps = vec![DiagStep { label: format!("Diagnosing {}...", name), status: DiagStatus::Running, detail: String::new() }];

        let (tx, rx) = mpsc::unbounded_channel();
        self.diag_rx = Some(rx);

        tokio::spawn(async move {
            let is_mac_check = Command::new("ssh").args([
                "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), "uname -s"
            ]).output().await;
            let is_mac = is_mac_check.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string() == "Darwin").unwrap_or(false);
            let pfx = if is_mac { "export PATH=/opt/homebrew/bin:$PATH; " } else { "" };

            // Step 1: SSH connectivity
            let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Running, detail: format!("ssh {}@{}", user, host) });
            let ssh_ok = tokio::time::timeout(Duration::from_secs(6),
                Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), "echo ok"]).output()
            ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);
            let _ = tx.send(DiagStep {
                label: "SSH connectivity".into(),
                status: if ssh_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if ssh_ok { "connected".into() } else { "unreachable — check Tailscale/network".into() },
            });
            if !ssh_ok { let _ = tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Fail, detail: "Cannot proceed without SSH".into() }); return; }

            // Step 2: Tailscale status
            let _ = tx.send(DiagStep { label: "Tailscale".into(), status: DiagStatus::Running, detail: String::new() });
            let ts_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), r#"tailscale status --self --json 2>/dev/null | grep -o '"Online":[a-z]*' | head -1 | cut -d: -f2 || echo ?"#
            ]).output().await;
            let ts_online = ts_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
            let ts_ok = ts_online == "True" || ts_online == "true";
            let _ = tx.send(DiagStep {
                label: "Tailscale".into(),
                status: if ts_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if ts_ok { "online".into() } else { format!("status: {}", ts_online) },
            });

            // Step 3: OpenClaw installed
            let _ = tx.send(DiagStep { label: "OpenClaw installed".into(), status: DiagStatus::Running, detail: String::new() });
            let oc_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), &format!("{}openclaw --version 2>/dev/null || echo NOT_INSTALLED", pfx)
            ]).output().await;
            let oc_ver = oc_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
            let oc_installed = !oc_ver.contains("NOT_INSTALLED") && oc_ver != "?";
            if !oc_installed && fix {
                let _ = tx.send(DiagStep { label: "OpenClaw installed".into(), status: DiagStatus::Running, detail: "installing...".into() });
                let install_cmd = if is_mac { format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx) }
                    else { "sudo npm install -g openclaw@latest 2>&1 | tail -1".into() };
                let _ = tokio::time::timeout(Duration::from_secs(120),
                    Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host), &install_cmd]).output()).await;
                let _ = tx.send(DiagStep { label: "OpenClaw installed".into(), status: DiagStatus::Fixed, detail: "installed".into() });
            } else {
                let _ = tx.send(DiagStep {
                    label: "OpenClaw installed".into(),
                    status: if oc_installed { DiagStatus::Pass } else { DiagStatus::Fail },
                    detail: if oc_installed { oc_ver } else { "not found — run with fix to install".into() },
                });
            }

            // Step 4: Gateway running
            let _ = tx.send(DiagStep { label: "Gateway running".into(), status: DiagStatus::Running, detail: String::new() });
            let gw_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), &format!("ss -tlnp 2>/dev/null | grep {} | head -1 || echo NONE", gw_port)
            ]).output().await;
            let gw_line = gw_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
            let gw_running = !gw_line.contains("NONE") && !gw_line.is_empty();
            if !gw_running && fix {
                let _ = tx.send(DiagStep { label: "Gateway running".into(), status: DiagStatus::Running, detail: "starting gateway...".into() });
                let start_cmd = if is_mac {
                    format!("{}nohup openclaw gateway start > /dev/null 2>&1 &", pfx)
                } else {
                    "sudo systemctl start openclaw-gateway 2>/dev/null || nohup openclaw gateway start > /dev/null 2>&1 &".into()
                };
                let _ = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &start_cmd]).output().await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = tx.send(DiagStep { label: "Gateway running".into(), status: DiagStatus::Fixed, detail: "started".into() });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Gateway running".into(),
                    status: if gw_running { DiagStatus::Pass } else { DiagStatus::Fail },
                    detail: if gw_running { format!("port {}", gw_port) } else { "not running".into() },
                });
            }

            // Step 5: Gateway API responding
            let _ = tx.send(DiagStep { label: "Gateway API".into(), status: DiagStatus::Running, detail: String::new() });
            let api_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), &format!("curl -s -m 3 http://localhost:{}/health 2>/dev/null || echo FAIL", gw_port)
            ]).output().await;
            let api_resp = api_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
            let api_ok = !api_resp.contains("FAIL") && !api_resp.is_empty();
            let _ = tx.send(DiagStep {
                label: "Gateway API".into(),
                status: if api_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if api_ok { "responding".into() } else { "not responding".into() },
            });

            // Step 6: Config file exists
            let _ = tx.send(DiagStep { label: "Config file".into(), status: DiagStatus::Running, detail: String::new() });
            let cfg_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), "test -f ~/.openclaw/openclaw.json && echo EXISTS || echo MISSING"
            ]).output().await;
            let cfg_exists = cfg_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "EXISTS").unwrap_or(false);
            let _ = tx.send(DiagStep {
                label: "Config file".into(),
                status: if cfg_exists { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if cfg_exists { "~/.openclaw/openclaw.json".into() } else { "missing".into() },
            });

            // Step 7: Workspace exists
            let _ = tx.send(DiagStep { label: "Agent workspace".into(), status: DiagStatus::Running, detail: String::new() });
            let ws_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), "ls ~/CLAUDE/clawd/SOUL.md 2>/dev/null && echo HAS_SOUL || echo NO_SOUL"
            ]).output().await;
            let has_soul = ws_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).contains("HAS_SOUL")).unwrap_or(false);
            let _ = tx.send(DiagStep {
                label: "Agent workspace".into(),
                status: if has_soul { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if has_soul { "SOUL.md found".into() } else { "no SOUL.md — agent may lack identity".into() },
            });

            // Done
            let passes = 7; // we'll count from received steps
            let _ = tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Pass, detail: "diagnostic complete".into() });
        });
    }

    /// Toggle a service plugin enabled/disabled via SSH
    fn toggle_service(&mut self) {
        if self.svc_selected >= self.svc_list.len() { return; }
        let svc = &self.svc_list[self.svc_selected];
        if svc.name == "model" || svc.name == "gateway" { return; } // Can't toggle these
        let new_state = !svc.enabled;
        let name = svc.name.clone();
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();

        let cmd = format!(
            r#"python3 -c "
import json
with open('$HOME/.openclaw/openclaw.json'.replace('$HOME', __import__('os').path.expanduser('~'))) as f:
    d = json.load(f)
d.setdefault('plugins', {{}}).setdefault('entries', {{}}).setdefault('{}', {{}})['enabled'] = {}
with open('$HOME/.openclaw/openclaw.json'.replace('$HOME', __import__('os').path.expanduser('~')), 'w') as f:
    json.dump(d, f, indent=2)
print('ok')
""#, name, if new_state { "True" } else { "False" }
        );

        let toast_msg = format!("{} {} {}", svc.icon, name, if new_state { "enabled" } else { "disabled" });
        self.toast(&toast_msg);

        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &cmd,
                ]).output()
            ).await;
        });

        // Optimistic update
        if let Some(svc) = self.svc_list.get_mut(self.svc_selected) {
            svc.enabled = new_state;
        }
    }

    /// Load workspace files for focused agent via SSH (non-blocking)
    fn start_workspace_load(&mut self) {
        if self.selected >= self.agents.len() { return; }
        if self.ws_loading { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        self.ws_loading = true;
        self.ws_content = None;
        self.ws_content_scroll = 0;

        let (tx, rx) = mpsc::unbounded_channel();
        self.ws_load_rx = Some(rx);

        tokio::spawn(async move {
            let check_cmd = AGENT_FILES.iter()
                .map(|(name, _, _)| format!("f=\"$(find ~ -maxdepth 3 -name '{}' -path '*/clawd/{}' 2>/dev/null | head -1)\"; if [ -n \"$f\" ]; then echo \"EXISTS:{}:$(stat -c%s \"$f\" 2>/dev/null || stat -f%z \"$f\" 2>/dev/null):$f\"; else echo \"MISSING:{}\"; fi", name, name, name, name))
                .collect::<Vec<_>>().join("; ");

            let output = tokio::time::timeout(
                Duration::from_secs(10),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &check_cmd,
                ]).output()
            ).await;

            let mut files = Vec::new();
            if let Ok(Ok(o)) = output {
                let stdout = String::from_utf8_lossy(&o.stdout);
                for line in stdout.lines() {
                    if let Some(rest) = line.strip_prefix("EXISTS:") {
                        let parts: Vec<&str> = rest.splitn(3, ':').collect();
                        if parts.len() >= 3 {
                            let name = parts[0];
                            let size: u64 = parts[1].parse().unwrap_or(0);
                            let path = parts[2];
                            if let Some((_, _, icon)) = AGENT_FILES.iter().find(|(n, _, _)| *n == name) {
                                files.push(WorkspaceFile { name: name.to_string(), path: path.to_string(), icon, exists: true, size_bytes: Some(size) });
                            }
                        }
                    } else if let Some(name) = line.strip_prefix("MISSING:") {
                        if let Some((_, _, icon)) = AGENT_FILES.iter().find(|(n, _, _)| *n == name) {
                            files.push(WorkspaceFile { name: name.to_string(), path: String::new(), icon, exists: false, size_bytes: None });
                        }
                    }
                }
            }
            if files.is_empty() {
                for (name, _, icon) in AGENT_FILES {
                    files.push(WorkspaceFile { name: name.to_string(), path: String::new(), icon, exists: false, size_bytes: None });
                }
            }

            // Crons
            let cron_output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), "openclaw cron list --json 2>/dev/null || echo '[]'",
                ]).output()
            ).await;
            let mut crons = Vec::new();
            if let Ok(Ok(o)) = cron_output {
                let stdout = String::from_utf8_lossy(&o.stdout);
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(stdout.trim()) {
                    for item in arr {
                        crons.push(CronEntry {
                            id: item["id"].as_str().unwrap_or("").to_string(),
                            schedule: item["schedule"].as_str().unwrap_or("").to_string(),
                            description: item["description"].as_str().unwrap_or(item["prompt"].as_str().unwrap_or("(no description)")).to_string(),
                            enabled: item["enabled"].as_bool().unwrap_or(true),
                        });
                    }
                }
            }

            let _ = tx.send((files, crons));
        });
    }

    /// Fetch file content via SSH (non-blocking)
    fn start_file_load(&mut self) {
        if self.ws_selected >= self.ws_files.len() { return; }
        let file = &self.ws_files[self.ws_selected];
        if !file.exists || file.path.is_empty() {
            self.ws_content = Some("(file not found on agent)".to_string());
            return;
        }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let path = file.path.clone();
        self.ws_content = Some("Loading...".to_string());

        let (tx, rx) = mpsc::unbounded_channel();
        self.ws_file_rx = Some(rx);

        tokio::spawn(async move {
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &format!("cat '{}'", path),
                ]).output()
            ).await;
            let content = match output {
                Ok(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
                Ok(Ok(o)) => format!("Error: {}", String::from_utf8_lossy(&o.stderr)),
                _ => "(timeout reading file)".to_string(),
            };
            let _ = tx.send(content);
        });
        self.ws_content_scroll = 0;
    }

    /// Save edited file content back to agent via SSH (non-blocking)
    fn start_file_save(&mut self) {
        if self.ws_selected >= self.ws_files.len() { return; }
        let file = &self.ws_files[self.ws_selected];
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let path = if file.path.is_empty() {
            format!("~/CLAUDE/clawd/{}", file.name)
        } else {
            file.path.clone()
        };

        let escaped_content = self.ws_edit_buffer.replace("'", "'\''");
        let cmd = format!("mkdir -p $(dirname '{}') && cat > '{}' << 'SAMEOF'
{}
SAMEOF", path, path, escaped_content);

        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &cmd,
                ]).output()
            ).await;
        });

        self.ws_editing = false;
        self.ws_content = Some(self.ws_edit_buffer.clone());
        let fname = if self.ws_selected < self.ws_files.len() { self.ws_files[self.ws_selected].name.clone() } else { "file".into() };
        self.toast(&format!("✓ Saved {}", fname));
    }

    fn start_refresh(&mut self) {
        if self.refreshing { return; }
        self.refreshing = true;
        self.last_refresh = Instant::now();
        let (tx, rx) = mpsc::unbounded_channel();
        self.refresh_rx = Some(rx);

        for (i, a) in self.agents.iter().enumerate() {
            let (host, user, sip) = (a.host.clone(), a.ssh_user.clone(), self.self_ip.clone());
            let tx = tx.clone();
            tokio::spawn(async move {
                let (status, os, kern, oc, lat, cpu, ram, disk, act, ctx) = probe_agent(&host, &user, &sip).await;
                let _ = tx.send(ProbeResult { index: i, status, os, kernel: kern, oc_version: oc, latency_ms: lat, cpu_pct: cpu, ram_pct: ram, disk_pct: disk, activity: act, context_pct: ctx });
            });
        }
    }

    fn drain_refresh_results(&mut self) -> Vec<(usize, AgentStatus, String, String, String, Option<u32>)> {
        let mut updates = vec![];
        if let Some(rx) = &mut self.refresh_rx {
            while let Ok(r) = rx.try_recv() {
                if r.index < self.agents.len() {
                    self.agents[r.index].status = r.status.clone();
                    if !r.os.is_empty() { self.agents[r.index].os = r.os.clone(); }
                    if !r.kernel.is_empty() { self.agents[r.index].kernel = r.kernel.clone(); }
                    if !r.oc_version.is_empty() { self.agents[r.index].oc_version = r.oc_version.clone(); }
                    self.agents[r.index].latency_ms = r.latency_ms;
                    self.agents[r.index].cpu_pct = r.cpu_pct;
                    self.agents[r.index].ram_pct = r.ram_pct;
                    self.agents[r.index].disk_pct = r.disk_pct;
                    self.agents[r.index].last_seen = now_str();
                    self.agents[r.index].last_probe_at = Some(Instant::now());
                    updates.push((r.index, r.status, r.os, r.kernel, r.oc_version, r.latency_ms));
                }
            }
        }
        if self.refreshing && self.last_refresh.elapsed() > Duration::from_secs(5) {
            self.refreshing = false;
        }
        updates
    }

    fn filtered_agents(&self) -> Vec<usize> {
        if self.filter_text.is_empty() {
            (0..self.agents.len()).collect()
        } else {
            let q = self.filter_text.to_lowercase();
            self.agents.iter().enumerate()
                .filter(|(_, a)| {
                    a.name.to_lowercase().contains(&q)
                    || a.db_name.to_lowercase().contains(&q)
                    || a.location.to_lowercase().contains(&q)
                    || a.host.contains(&q)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }

    fn check_alerts(&mut self) {
        let now = now_str();
        for a in &self.agents {
            if a.status == AgentStatus::Offline && !a.last_seen.is_empty() {
                // Only alert if we haven't recently alerted for this agent
                let already = self.alerts.iter().any(|al| al.agent == a.db_name && al.message.contains("offline"));
                if !already {
                    self.alerts.push(Alert {
                        time: now.clone(), agent: a.db_name.clone(), emoji: a.emoji.clone(),
                        message: format!("{} went offline", a.name),
                        severity: AlertSeverity::Critical,
                    });
                    self.alert_flash = Some(Instant::now());
                }
            }
            if let Some(disk) = a.disk_pct {
                if disk > 90.0 {
                    let already = self.alerts.iter().any(|al| al.agent == a.db_name && al.message.contains("disk"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(), agent: a.db_name.clone(), emoji: a.emoji.clone(),
                            message: format!("{} disk at {:.0}%", a.name, disk),
                            severity: AlertSeverity::Warning,
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
            if let Some(ram) = a.ram_pct {
                if ram > 90.0 {
                    let already = self.alerts.iter().any(|al| al.agent == a.db_name && al.message.contains("RAM"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(), agent: a.db_name.clone(), emoji: a.emoji.clone(),
                            message: format!("{} RAM at {:.0}%", a.name, ram),
                            severity: AlertSeverity::Warning,
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
        }
        // Keep last 100 alerts
        if self.alerts.len() > 100 { self.alerts.drain(0..self.alerts.len()-100); }
    }

    fn update_status_bar(&mut self) {
        let on = self.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
        let total = self.agents.len();
        let spinner_chars = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
        // Always advance spinner for a live "app is alive" indicator
        self.spinner_frame = (self.spinner_frame + 1) % spinner_chars.len();
        let refresh = format!(" {} ", spinner_chars[self.spinner_frame]);
        let chat_count = self.chat_history.len();
        let sel_info = if !self.multi_selected.is_empty() {
            format!(" │ 🔲 {}", self.multi_selected.len())
        } else { String::new() };
        let alert_info = if !self.alerts.is_empty() {
            let crits = self.alerts.iter().filter(|a| a.severity == AlertSeverity::Critical).count();
            if crits > 0 { format!(" │ 🔴 {} alerts", self.alerts.len()) }
            else { format!(" │ 🟡 {} alerts", self.alerts.len()) }
        } else { String::new() };
        self.status_message = format!(
            "v1.2 │ {}/{} online{}{}{} │ sort:{} │ chat({}) │ {}/{} │ /=cmd ?=help",
            on, total, refresh, sel_info, alert_info, self.sort_mode.label(), chat_count,
            self.theme_name.label(), self.bg_density.label()
        );
    }
}

// ---- SSH Probe ----

async fn probe_agent(host: &str, user: &str, self_ip: &str) -> (AgentStatus, String, String, String, Option<u32>, Option<f32>, Option<f32>, Option<f32>, String, Option<f32>) {
    let start = Instant::now();
    if host == "localhost" || host == self_ip {
        let os = Command::new("bash").args(["-c", ". /etc/os-release 2>/dev/null && echo \"$NAME $VERSION_ID\" || echo unknown"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let kern = Command::new("uname").arg("-r").output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let oc = Command::new("bash").args(["-c", "openclaw --version 2>/dev/null || echo ?"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let cpu = Command::new("bash").args(["-c", r#"top -bn1 2>/dev/null | grep 'Cpu(s)' | awk '{print $2+$4}'"#]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f32>().ok()).ok().flatten();
        let ram = Command::new("bash").args(["-c", r#"free 2>/dev/null | awk '/Mem:/{printf "%.1f", $3/$2*100}'"#]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f32>().ok()).ok().flatten();
        let disk = Command::new("bash").args(["-c", r#"df / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $5}'"#]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f32>().ok()).ok().flatten();
        let ms = start.elapsed().as_millis() as u32;
        return (AgentStatus::Online, os, kern, oc, Some(ms), cpu, ram, disk, "local".into(), None);
    }
    let tgt = format!("{}@{}", user, host);
    let script = r#"OS=$(. /etc/os-release 2>/dev/null && echo "$NAME $VERSION_ID" || (sw_vers -productName 2>/dev/null; sw_vers -productVersion 2>/dev/null) || echo ?); KERN=$(uname -r); OC=$(openclaw --version 2>/dev/null || echo ?); CPU=$(top -bn1 2>/dev/null | grep 'Cpu(s)' | awk '{print $2+$4}' || echo ?); RAM=$(free 2>/dev/null | awk '/Mem:/{printf "%.1f", $3/$2*100}' || vm_stat 2>/dev/null | awk '/Pages active/{a=$NF} /Pages wired/{w=$NF} /Pages free/{f=$NF} END{if(a+w+f>0) printf "%.1f",(a+w)/(a+w+f)*100; else print "?"}'); DISK=$(df / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $5}' || echo ?); echo "OS:$OS"; echo "KERN:$KERN"; echo "OC:$OC"; echo "CPU:$CPU"; echo "RAM:$RAM"; echo "DISK:$DISK"; ACT=$(openclaw status --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);ss=d.get('sessions',[]);active=[s for s in ss if s.get('active')];print(active[0].get('channel','idle') if active else 'idle')" 2>/dev/null || echo idle); CTX=$(openclaw status --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);ss=d.get('sessions',[]);active=[s for s in ss if s.get('active')];t=active[0].get('contextTokens',0) if active else 0;m=active[0].get('maxTokens',1000000) if active else 1000000;print(f'{t/m*100:.1f}')" 2>/dev/null || echo ?); echo "ACT:$ACT"; echo "CTX:$CTX""#;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("ssh").args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",&tgt,"bash","-c",script]).output()
    ).await;
    let result = match result {
        Ok(r) => r,
        Err(_) => return (AgentStatus::Offline, String::new(), String::new(), String::new(), None, None, None, None, String::new(), None),
    };
    match result {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let (mut os, mut kern, mut oc) = (String::new(), String::new(), String::new());
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("OS:") { os = v.trim().into(); }
                else if let Some(v) = l.strip_prefix("KERN:") { kern = v.trim().into(); }
                else if let Some(v) = l.strip_prefix("OC:") { oc = v.trim().into(); }
            }
            let (mut cpu, mut ram, mut disk, mut act, mut ctx) = (None, None, None, String::new(), None);
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("CPU:") { cpu = v.trim().parse::<f32>().ok(); }
                else if let Some(v) = l.strip_prefix("RAM:") { ram = v.trim().parse::<f32>().ok(); }
                else if let Some(v) = l.strip_prefix("DISK:") { disk = v.trim().parse::<f32>().ok(); }
                else if let Some(v) = l.strip_prefix("ACT:") { act = v.trim().to_string(); }
                else if let Some(v) = l.strip_prefix("CTX:") { ctx = v.trim().parse::<f32>().ok(); }
            }
            let ms = start.elapsed().as_millis() as u32;
            (AgentStatus::Online, os, kern, oc, Some(ms), cpu, ram, disk, act, ctx)
        }
        _ => (AgentStatus::Offline, String::new(), String::new(), String::new(), None, None, None, None, String::new(), None),
    }
}

fn now_str() -> String {
    use std::process::Command as C;
    C::new("date").arg("+%H:%M:%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("now".into())
}

// ---- Chat Line Rendering ----

const BRAILLE_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Agent workspace file metadata
#[derive(Clone, Debug)]
struct WorkspaceFile {
    name: String,
    path: String,
    icon: &'static str,
    exists: bool,
    size_bytes: Option<u64>,
}

/// Cron job entry from OpenClaw
#[derive(Clone, Debug)]
struct CronEntry {
    id: String,
    schedule: String,
    description: String,
    enabled: bool,
}

/// OpenClaw service/plugin entry
#[derive(Clone, Debug)]
struct ServiceEntry {
    name: String,
    icon: &'static str,
    enabled: bool,
    has_channel_config: bool,
    summary: String,  // e.g. "2 groups, dmPolicy: pairing"
}

/// Diagnostic step result
#[derive(Clone, Debug)]
struct DiagStep {
    label: String,
    status: DiagStatus,
    detail: String,
}

#[derive(Clone, Debug, PartialEq)]
enum DiagStatus {
    Running,
    Pass,
    Fail,
    Fixed,
    Skipped,
}

impl DiagStatus {
    fn icon(&self) -> &'static str {
        match self { DiagStatus::Running => "⏳", DiagStatus::Pass => "✓", DiagStatus::Fail => "✗", DiagStatus::Fixed => "🔧", DiagStatus::Skipped => "⊘" }
    }
}

const SERVICE_ICONS: &[(&str, &str)] = &[
    ("telegram", "📱"), ("discord", "🎮"), ("signal", "🔒"), ("whatsapp", "💬"),
    ("slack", "💼"), ("irc", "📟"), ("matrix", "🔷"), ("imessage", "🍎"),
    ("bluebubbles", "🫧"), ("msteams", "🏢"), ("nostr", "🟣"), ("twitch", "🎬"),
    ("line", "🟢"), ("googlechat", "🟡"), ("mattermost", "🔵"), ("feishu", "🦅"),
    ("zalo", "📲"), ("nextcloud-talk", "☁️"), ("tlon", "🌐"),
];

fn svc_icon(name: &str) -> &'static str {
    SERVICE_ICONS.iter().find(|(n, _)| *n == name).map(|(_, i)| *i).unwrap_or("🔌")
}

const AGENT_FILES: &[(&str, &str, &str)] = &[
    ("SOUL.md", "soul", "🧬"),
    ("IDENTITY.md", "identity", "🪪"),
    ("AGENTS.md", "agents", "📋"),
    ("MEMORY.md", "memory", "🧠"),
    ("USER.md", "user", "👤"),
    ("HEARTBEAT.md", "heartbeat", "💓"),
    ("TOOLS.md", "tools", "🔧"),
    ("HEARTBEAT_TASKS.md", "tasks", "📝"),
    ("RECALL.md", "recall", "🔍"),
    ("CHECKPOINT.md", "checkpoint", "📌"),
];

/// Result of a background chat poll
struct ChatPollResult {
    global: Vec<ChatLine>,
    agent: Option<Vec<ChatLine>>,
    to_route: Vec<(String, String)>,
    new_routed_ids: Vec<i64>,
}

/// Minimum content width (chars) for message word-wrap, preventing extremely narrow wrapping.
const MIN_WRAP_WIDTH: usize = 10;
/// Approximate lines rendered per chat message (header + body + blank), used to estimate
/// the number of unseen messages from a raw line count.
const LINES_PER_MSG_EST: usize = 3;
/// Spinner frame duration in milliseconds.  Dividing subsecond millis by this value gives
/// a 0–9 index that advances ~10 times per second.
const SPINNER_FRAME_MS: u64 = 100;
/// Input poll interval in milliseconds. Lower values improve key/menu responsiveness.
const INPUT_POLL_MS: u64 = 10;

fn fmt_hhmm(t: &str) -> String {
    t.chars().take(5).collect()
}

fn build_chat_lines(messages: &[ChatLine], user: &str, t: &Theme, area_width: u16, spinner_frame: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if messages.is_empty() {
        lines.push(Line::from(""));
        let empty_art = vec![
            "",
            "      ╭───────────────────╮",
            "      │   📡  Listening... │",
            "      │                   │",
            "      │  Tab to chat,     │",
            "      │  type a message   │",
            "      │  and hit Enter    │",
            "      ╰───────────────────╯",
        ];
        for l in empty_art {
            lines.push(Line::from(Span::styled(l.to_string(), Style::default().fg(t.text_dim))));
        }
        return lines;
    }

    let inner_w = area_width.saturating_sub(2) as usize;
    let wrap_w = inner_w.saturating_sub(8).max(20);

    for msg in messages {
        let ts = fmt_hhmm(&msg.time);
        let is_outgoing = msg.sender == user;

        if is_outgoing {
            // Right-aligned outgoing message (operator)
            let tgt = msg.target.as_ref()
                .map(|tgt| format!(" → @{}", tgt))
                .unwrap_or_else(|| " → all".into());
            let header_content = format!("{}{}   {}", msg.sender, tgt, ts);
            let hlen = header_content.chars().count();
            let hpad = inner_w.saturating_sub(hlen);
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(hpad)),
                Span::styled(format!("{}{}", msg.sender, tgt), Style::default().fg(t.sender_self).bold()),
                Span::styled(format!("   {}", ts), Style::default().fg(t.text_dim)),
            ]));

            // Word-wrap body and right-align each line
            let body_wrap = wrap_w.saturating_sub(2).max(MIN_WRAP_WIDTH);
            let words: Vec<&str> = msg.message.split_whitespace().collect();
            let mut wrapped: Vec<String> = Vec::new();
            let mut cur = String::new();
            for w in &words {
                if !cur.is_empty() && cur.chars().count() + w.len() + 1 > body_wrap {
                    wrapped.push(cur.clone());
                    cur.clear();
                }
                if !cur.is_empty() { cur.push(' '); }
                cur.push_str(w);
            }
            if !cur.is_empty() { wrapped.push(cur); }
            if wrapped.is_empty() { wrapped.push(msg.message.clone()); }

            for bl in &wrapped {
                let blen = bl.chars().count();
                let bpad = inner_w.saturating_sub(blen + 2);
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(bpad)),
                    Span::styled(bl.clone(), Style::default().fg(t.text)),
                    Span::raw("  "),
                ]));
            }

            // Status indicator (right-aligned)
            let st_icon: String = match msg.status.as_str() {
                "responded" => "✓✓".into(),
                "streaming" => {
                    let c = BRAILLE_SPINNER[spinner_frame % BRAILLE_SPINNER.len()];
                    format!("{} streaming", c)
                }
                "connecting" => {
                    let dots = ".".repeat((spinner_frame % 3) + 1);
                    format!("⚡ connecting{}", dots)
                }
                "thinking" | "processing" => {
                    let c = BRAILLE_SPINNER[spinner_frame % BRAILLE_SPINNER.len()];
                    format!("{} thinking", c)
                }
                "pending" => "⏳ sending".into(),
                "failed" => "✗ failed".into(),
                _ => if msg.response.is_some() { "✓✓".into() } else { "✓".into() },
            };
            let spad = inner_w.saturating_sub(st_icon.chars().count() + 1);
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(spad)),
                Span::styled(st_icon, Style::default().fg(t.text_dim)),
                Span::raw(" "),
            ]));

            // Show agent response below outgoing message (left-aligned reply)
            if let Some(resp) = &msg.response {
                let responder = msg.target.as_ref().map(|s| s.as_str()).unwrap_or("agent");
                let avatar = responder.chars().next()
                    .map(|c| c.to_ascii_uppercase()).unwrap_or('?');
                lines.push(Line::from(vec![
                    Span::styled(format!("  [{}] ", avatar), Style::default().fg(t.sender_other).bold()),
                    Span::styled(responder.to_string(), Style::default().fg(t.sender_other).bold()),
                ]));
                let words: Vec<&str> = resp.split_whitespace().collect();
                let mut cur = String::new();
                let mut first = true;
                let body_wrap = wrap_w.saturating_sub(2).max(20);
                for w in &words {
                    if !cur.is_empty() && cur.chars().count() + w.len() + 1 > body_wrap {
                        let prefix = if first { "  ↳ " } else { "    " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                            Span::styled(cur.clone(), Style::default().fg(t.response)),
                        ]));
                        cur.clear();
                        first = false;
                    }
                    if !cur.is_empty() { cur.push(' '); }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    let prefix = if first { "  ↳ " } else { "    " };
                    // Add blinking cursor if still streaming
                    let is_streaming = msg.status == "streaming";
                    let cursor = if is_streaming { "▌" } else { "" };
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                        Span::styled(cur, Style::default().fg(t.response)),
                        Span::styled(cursor.to_string(), Style::default().fg(t.accent)),
                    ]));
                }
            }
        } else {
            // Left-aligned incoming message (agent)
            let avatar = msg.sender.chars().next()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or('?');
            lines.push(Line::from(vec![
                Span::styled(format!("  [{}] ", avatar), Style::default().fg(t.sender_other).bold()),
                Span::styled(msg.sender.clone(), Style::default().fg(t.sender_other).bold()),
                Span::styled(format!("   {}", ts), Style::default().fg(t.text_dim)),
            ]));

            if let Some(resp) = &msg.response {
                // Word-wrapped response
                let words: Vec<&str> = resp.split_whitespace().collect();
                let mut cur = String::new();
                let mut first = true;
                for w in &words {
                    if !cur.is_empty() && cur.chars().count() + w.len() + 1 > wrap_w {
                        let prefix = if first { "  ↳ " } else { "    " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                            Span::styled(cur.clone(), Style::default().fg(t.response)),
                        ]));
                        cur.clear();
                        first = false;
                    }
                    if !cur.is_empty() { cur.push(' '); }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    let prefix = if first { "  ↳ " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                        Span::styled(cur, Style::default().fg(t.response)),
                    ]));
                }
            } else {
                let status_text: String = match msg.status.as_str() {
                    "streaming" => {
                        let c = BRAILLE_SPINNER[spinner_frame % BRAILLE_SPINNER.len()];
                        format!("  {} tokens flowing...", c)
                    }
                    "thinking" => {
                        let c = BRAILLE_SPINNER[spinner_frame % BRAILLE_SPINNER.len()];
                        format!("  {} agent is thinking...", c)
                    }
                    "connecting" => {
                        let dots = ".".repeat((spinner_frame % 3) + 1);
                        format!("  ⚡ connecting{}", dots)
                    }
                    "pending" | "processing" => "  ⏳ sending...".into(),
                    "received" => "  📨 received".into(),
                    _ => String::new(),
                };
                if !status_text.is_empty() {
                    lines.push(Line::from(Span::styled(status_text, Style::default().fg(t.pending))));
                }
            }
        }

        lines.push(Line::from(""));
    }
    lines
}

// ---- Rendering ----


// ---- Responsive Layout Helpers ----

fn is_narrow(area: &Rect) -> bool { area.width < 80 }
fn is_wide(area: &Rect) -> bool { area.width > 160 }

fn dashboard_split(area: &Rect) -> (Constraint, Constraint) {
    if is_narrow(area) { (Constraint::Percentage(100), Constraint::Percentage(0)) }
    else if is_wide(area) { (Constraint::Percentage(40), Constraint::Percentage(60)) }
    else { (Constraint::Percentage(50), Constraint::Percentage(50)) }
}

fn detail_split(area: &Rect) -> (Constraint, Constraint) {
    if is_narrow(area) { (Constraint::Percentage(100), Constraint::Percentage(0)) }
    else if is_wide(area) { (Constraint::Percentage(35), Constraint::Percentage(65)) }
    else { (Constraint::Percentage(40), Constraint::Percentage(60)) }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() }
    else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}

fn os_ascii_art(os: &str) -> &'static [&'static str] {
    let os_lower = os.to_lowercase();
    if os_lower.contains("mac") || os_lower.contains("darwin") {
        &[
            "       .:'",
            "    _ :'_",
            " .`_`-'_`.",
            ":________.-'",
            "`-._._._.'",
        ]
    } else if os_lower.contains("ubuntu") {
        &[
            "   _____",
            "  /  __ \\",
            " | /  \\/ |",
            " | \\__/  |",
            "  \\____/",
        ]
    } else if os_lower.contains("arch") {
        &[
            "      /\\",
            "     /  \\",
            "    / /\\ \\",
            "   / /  \\ \\",
            "  /_/    \\_\\",
        ]
    } else if os_lower.contains("fedora") {
        &[
            "   ,''''.",
            "  |   ,--'",
            "  |  |",
            "  |  '---.",
            "   '----'",
        ]
    } else if os_lower.contains("linux") {
        &[
            "    ___",
            "   (.. |",
            "   (<> |",
            "  / __  \\",
            " ( /  \\ )|",
        ]
    } else if os_lower.contains("windows") {
        &[
            "  ,--.--.",
            " |  |  |",
            " |  |  |",
            " '------'",
            "  \\\\  //",
        ]
    } else {
        &[
            "  .------.",
            " | SERVER |",
            " |        |",
            " '--------'",
            "    |  |",
        ]
    }
}


fn render_too_small(frame: &mut Frame) {
    let area = frame.area();
    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled("Terminal too small", Style::default().fg(Color::Red).bold())),
        Line::from(Span::styled(format!("Need 60x20, got {}x{}", area.width, area.height), Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("Resize your terminal", Style::default().fg(Color::DarkGray))),
    ]).alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));
    frame.render_widget(msg, area);
}

fn render_splash(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();
    let bg = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg, area);

    let ver_line = format!("    v{} — {} agents in fleet", env!("CARGO_PKG_VERSION"), app.agents.len());
    let online_line = format!("    {} online", app.agents.iter().filter(|a| a.status == AgentStatus::Online).count());
    let logo: Vec<&str> = vec![
        "",
        r"    ____    _    __  __ ",
        r"   / ___|  / \  |  \/  |",
        r"   \___ \ / _ \ | |\/| |",
        r"    ___) / ___ \| |  | |",
        r"   |____/_/   \_\_|  |_|",
        "",
        "",
        "    S . A . M   M I S S I O N   C O N T R O L",
        "",
        &ver_line,
        &online_line,
        "",
        "    Strange Artificial Machine — Fleet Orchestration TUI",
        "",
        "    Press any key to continue...",
    ];

    // Animated gradient: cycle through blue shades using elapsed time
    let elapsed_ms = app.splash_start.elapsed().as_millis() as u32;
    let phase = (elapsed_ms / 80) % 6;
    let gradient_colors = [
        Color::Rgb(40, 140, 220),
        Color::Rgb(60, 170, 255),
        Color::Rgb(80, 200, 255),
        Color::Rgb(100, 220, 255),
        Color::Rgb(80, 200, 255),
        Color::Rgb(60, 170, 255),
    ];

    let cy = area.height / 2;
    let start_y = cy.saturating_sub(logo.len() as u16 / 2);

    for (i, line) in logo.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.height { break; }
        let color = if i >= 1 && i <= 5 {
            // Animated gradient on logo lines
            gradient_colors[((i as u32 + phase) % 6) as usize]
        } else if i == 8 {
            t.header_title
        } else {
            t.text_dim
        };
        let p = Paragraph::new(Line::from(Span::styled(line.to_string(), Style::default().fg(color).bold())))
            .alignment(Alignment::Center);
        frame.render_widget(p, Rect::new(0, y, area.width, 1));
    }
}

fn render_dashboard(frame: &mut Frame, app: &mut App) {
    if frame.area().width < 60 || frame.area().height < 20 { render_too_small(frame); return; }
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    // Clear with bg color
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let online = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
    let total = app.agents.len();
    let live = app.last_refresh.elapsed() < Duration::from_secs(60);
    let total_tokens: i32 = app.agents.iter().map(|a| a.token_burn).sum();
    let health_pct = if total > 0 { online * 100 / total } else { 0 };
    let health_color = if health_pct >= 80 { t.status_online } else if health_pct >= 50 { t.status_busy } else { t.status_offline };

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🛰️  S.A.M MISSION CONTROL", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("{}", online), Style::default().fg(t.status_online).bold()),
        Span::styled(format!("/{} agents", total), Style::default().fg(t.text_dim)),
        Span::raw("    "),
        Span::styled(format!("{}% healthy", health_pct), Style::default().fg(health_color)),
        Span::raw("    "),
        Span::styled(format!("{}tok", total_tokens), Style::default().fg(t.text_dim)),
        Span::raw("    "),
        Span::styled(if live { "● live" } else { "○ stale" }, Style::default().fg(if live { t.status_online } else { t.status_offline })),
        Span::raw("    "),
        Span::styled(if app.refreshing { "⟳ refreshing" } else { "" }, Style::default().fg(t.accent)),
        if app.alert_flash.map(|f| f.elapsed() < Duration::from_secs(5)).unwrap_or(false) {
            Span::styled("  ⚠️ NEW ALERT", Style::default().fg(t.status_offline).bold())
        } else { Span::raw("") },
        Span::raw("    "),
        Span::styled(chrono_now(), Style::default().fg(t.text_dim)),
        Span::raw("    "),
        Span::styled(match app.focus {
            Focus::Fleet => "▌Fleet▐", Focus::Chat => "▌Chat▐", _ => "▌Fleet▐",
        }, Style::default().fg(t.accent).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Double)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(header, outer[0]);

    let (fleet_pct, chat_pct) = dashboard_split(&outer[1]);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([fleet_pct, chat_pct])
        .split(outer[1]);

    app.fleet_area = body[0];
    app.chat_area = body[1];
    render_fleet_table(frame, app, body[0], app.focus == Focus::Fleet);
    if !is_narrow(&outer[1]) {
        render_chat_panel(frame, app, body[1], app.focus == Focus::Chat, false);
    }
    render_footer(frame, app, outer[2]);
}

fn mini_bar(pct: Option<f32>, width: usize) -> String {
    match pct {
        Some(p) => {
            let clamped = p.clamp(0.0, 100.0);
            let filled = ((clamped / 100.0) * width as f32).round() as usize;
            let bar: String = "█".repeat(filled) + &"░".repeat(width.saturating_sub(filled));
            format!("{:>3.0}%{}", clamped, bar)
        }
        None => format!("{:>4}{}", "—", "░".repeat(width)),
    }
}

fn mini_bar_color(pct: Option<f32>, t: &Theme, warn: f32, crit: f32) -> Color {
    match pct {
        Some(p) if p >= crit => t.status_offline,
        Some(p) if p >= warn => t.status_busy,
        Some(_) => t.status_online,
        None => t.text_dim,
    }
}

fn render_fleet_table(frame: &mut Frame, app: &mut App, area: Rect, active: bool) {
    let t = &app.theme;
    let fb = if active { t.border_active } else { t.border };

    let show_latency = area.width > 70;
    let show_resources = area.width > 120;
    let show_ip = area.width > 85;
    // Last Seen is shown in the 101-120 col range; at >120 the CPU/RAM/Disk resource columns take precedence
    let show_activity = area.width > 100;
    let hcells_vec: Vec<&str> = if show_resources {
        vec!["  ", "Agent", "IP", "Location", "Status", "Ping", "Activity", "Ctx%", "CPU", "RAM", "Disk", "Version"]
    } else if show_activity {
        vec!["  ", "Agent", "IP", "Location", "Status", "Ping", "Uptime", "Activity", "Version"]
    } else if show_ip && show_latency {
        vec!["  ", "Agent", "IP", "Location", "Status", "Ping", "Uptime", "Version"]
    } else if show_latency {
        vec!["  ", "Agent", "Location", "Status", "Ping", "Uptime", "Version"]
    } else {
        vec!["  ", "Agent", "Location", "Status", "Version"]
    };
    let hcells = hcells_vec.iter().map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.agents.iter().enumerate().map(|(i, a)| {
        let sel = i == app.selected && active;
        let bg = if sel { t.selected_bg } else if i % 2 == 1 { ratatui::style::Color::Rgb(20, 22, 28) } else { app.bg_density.bg() };
        let loc_color = match a.location.as_str() {
            "Home" => t.loc_home, "SM" => t.loc_sm, "VPS" => t.loc_vps, "Mobile" => t.loc_mobile, _ => t.text,
        };
        let st_color = match a.status {
            AgentStatus::Online => t.status_online, AgentStatus::Busy => t.status_busy,
            AgentStatus::Offline => t.status_offline, _ => t.text_dim,
        };
        let is_multi = app.multi_selected.contains(&i);
        let cursor = if sel && is_multi { "▶✓" } else if sel { "▶ " } else if is_multi { " ✓" } else { "  " };
        let lat_str = match a.latency_ms {
            Some(ms) => format!("{}ms", ms),
            None => "—".into(),
        };
        let lat_color = match a.latency_ms {
            Some(ms) if ms < 100 => t.status_online,
            Some(ms) if ms < 500 => t.status_busy,
            Some(_) => t.status_offline,
            None => t.text_dim,
        };
        let mut cells = vec![
            Cell::from(format!("{}{}", cursor, a.emoji)),
            Cell::from(a.name.clone()).style(Style::default().fg(t.text_bold).bold()),
        ];
        if show_ip {
            cells.push(Cell::from(a.host.clone()).style(Style::default().fg(t.accent2)));
        }
        cells.extend(vec![
            Cell::from(a.location.clone()).style(Style::default().fg(loc_color)),
            Cell::from(a.status.to_string()).style(Style::default().fg(st_color)),
        ]);
        if show_latency {
            cells.push(Cell::from(lat_str).style(Style::default().fg(lat_color)));
            cells.push(Cell::from(format_uptime(a.uptime_seconds)).style(Style::default().fg(t.text_dim)));
        }
        if show_activity && !show_resources {
            let act_display = if a.activity.is_empty() || a.activity == "idle" { "idle".to_string() } else { a.activity.clone() };
            let act_color = if act_display == "idle" { t.text_dim } else { t.accent };
            cells.push(Cell::from(act_display).style(Style::default().fg(act_color)));
        }
        if show_resources {
            let act_short = if a.activity.is_empty() || a.activity == "idle" { "·" } else { &a.activity };
            let act_color = if act_short == "·" { t.text_dim } else { t.accent };
            cells.push(Cell::from(act_short.chars().take(10).collect::<String>()).style(Style::default().fg(act_color)));
            let ctx_str = a.context_pct.map(|p| format!("{:.0}%", p)).unwrap_or("—".into());
            let ctx_color = match a.context_pct {
                Some(p) if p > 80.0 => t.status_offline,
                Some(p) if p > 50.0 => t.status_busy,
                Some(_) => t.status_online,
                None => t.text_dim,
            };
            cells.push(Cell::from(ctx_str).style(Style::default().fg(ctx_color)));
            cells.push(Cell::from(mini_bar(a.cpu_pct, 4)).style(Style::default().fg(mini_bar_color(a.cpu_pct, t, 70.0, 90.0))));
            cells.push(Cell::from(mini_bar(a.ram_pct, 4)).style(Style::default().fg(mini_bar_color(a.ram_pct, t, 70.0, 85.0))));
            cells.push(Cell::from(mini_bar(a.disk_pct, 4)).style(Style::default().fg(mini_bar_color(a.disk_pct, t, 80.0, 90.0))));
        }
        cells.push(Cell::from(a.oc_version.clone()).style(Style::default().fg(t.version)));
        Row::new(cells).style(Style::default().bg(bg)).height(1)
    }).collect();

    app.fleet_row_start_y = area.y + 1; // +1 for border, +1 for header handled in click calc

    let widths = if show_resources {
        vec![Constraint::Length(5), Constraint::Length(14), Constraint::Length(13), Constraint::Length(8), Constraint::Length(12), Constraint::Length(7), Constraint::Length(10), Constraint::Length(5), Constraint::Length(6), Constraint::Length(6), Constraint::Length(6), Constraint::Min(10)]
    } else if show_activity {
        vec![Constraint::Length(5), Constraint::Length(14), Constraint::Length(13), Constraint::Length(8), Constraint::Length(12), Constraint::Length(7), Constraint::Length(8), Constraint::Length(12), Constraint::Min(10)]
    } else if show_ip && show_latency {
        vec![Constraint::Length(5), Constraint::Length(14), Constraint::Length(13), Constraint::Length(8), Constraint::Length(12), Constraint::Length(7), Constraint::Length(8), Constraint::Min(10)]
    } else if show_latency {
        vec![Constraint::Length(5), Constraint::Length(14), Constraint::Length(8), Constraint::Length(12), Constraint::Length(7), Constraint::Length(8), Constraint::Min(10)]
    } else {
        vec![Constraint::Length(5), Constraint::Length(14), Constraint::Length(8), Constraint::Length(12), Constraint::Min(10)]
    };
    let fleet_title = if app.filter_active {
        if app.filter_text.is_empty() {
            " ◆── Fleet 🔍 (type to search) ──◆ ".to_string()
        } else {
            format!(" ◆── Fleet 🔍 {} ──◆ ", app.filter_text)
        }
    } else {
        format!(" ◆── Fleet [{}{}] ──◆ ", app.sort_mode.label(), app.sort_mode.arrow())
    };
    let table = Table::new(rows, widths).header(hrow)
    .block(Block::default().title(Span::styled(fleet_title, Style::default().fg(fb).bold()))
        .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(fb))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, area);
}

fn render_chat_panel(frame: &mut Frame, app: &App, area: Rect, active: bool, agent_mode: bool) {
    let t = &app.theme;
    let cb = if active { t.border_active } else { t.border };

    let cl = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    // Time-based spinner frame for typing animation (advances once per SPINNER_FRAME_MS).
    let spin_frame = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis() as u64 / SPINNER_FRAME_MS) as usize;

    let (messages, scroll, input_text) = if agent_mode {
        let msgs = app.agent_chat_lines();
        let lines = build_chat_lines(msgs, &app.user(), t, cl[0].width, spin_frame);
        (lines, app.agent_chat_scroll, &app.agent_chat_input)
    } else {
        let lines = build_chat_lines(&app.chat_history, &app.user(), t, cl[0].width, spin_frame);
        (lines, app.chat_scroll, &app.chat_input)
    };

    let vh = cl[0].height.saturating_sub(2) as usize;
    let tl = messages.len();
    let scroll_pos = if tl > vh && scroll == 0 { (tl - vh) as u16 } else { scroll };

    // Scroll indicator: count lines below the current viewport
    let lines_below = tl.saturating_sub(scroll_pos as usize + vh);
    let new_indicator = if lines_below > 0 {
        format!(" ▼ {} new ", (lines_below / LINES_PER_MSG_EST).max(1))
    } else {
        String::new()
    };

    let title = if agent_mode {
        let base = format!(" {} {} Chat", app.agents[app.selected].emoji, app.agents[app.selected].name);
        format!("{}{} ", base, new_indicator)
    } else {
        let count = app.chat_history.len();
        let base = if count > 0 { format!(" Chat ({})", count) } else { " Chat".to_string() };
        format!("{}{} ", base, new_indicator)
    };

    let chat = Paragraph::new(messages).scroll((scroll_pos, 0))
        .block(Block::default().title(Span::styled(title, Style::default().fg(cb).bold()))
            .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(cb))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(chat, cl[0]);

    let prompt = if agent_mode {
        format!(" @{} › ", app.agents[app.selected].db_name)
    } else if app.focus == Focus::Command {
        " ⚡ fleet command (runs on all agents) ⏎ ".to_string()
    } else if active {
        " broadcast to all ⏎ ".to_string()
    } else {
        " Tab to chat ".to_string()
    };

    let display_text = if !agent_mode && app.focus == Focus::Command { &app.command_input } else { input_text };
    let is_active = active || (!agent_mode && app.focus == Focus::Command);
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(t.accent)),
        Span::styled(display_text, Style::default().fg(t.text)),
        if is_active { Span::styled("▌", Style::default().fg(t.accent)) } else { Span::raw("") },
    ])).block(Block::default().title(prompt)
        .borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(if is_active { t.border_active } else { t.border }))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(input, cl[1]);
}

fn render_detail(frame: &mut Frame, app: &mut App) {
    let t = &app.theme;
    let a = &app.agents[app.selected];
    let chunks = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    // BG
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    // Header
    let st_color = match a.status {
        AgentStatus::Online => t.status_online, AgentStatus::Busy => t.status_busy,
        AgentStatus::Offline => t.status_offline, _ => t.text_dim,
    };
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{} {}", a.emoji, a.name), Style::default().fg(t.header_title).bold()),
        Span::raw("  —  "),
        Span::styled(a.status.to_string(), Style::default().fg(st_color)),
        Span::raw("    "),
        Span::styled(match app.focus {
            Focus::AgentChat => " 1:Info 2:▌Chat▐ 3:Files 4:Tasks 5:Svc",
            Focus::Workspace => " 1:Info 2:Chat 3:▌Files▐ 4:Tasks 5:Svc",
            Focus::Services => " 1:Info 2:Chat 3:Files 4:Tasks 5:▌Svc▐",
            _ => " 1:▌Info▐ 2:Chat 3:Files 4:Tasks 5:Svc",
        }, Style::default().fg(t.accent).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Double)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(header, chunks[0]);

    // Body: info left, chat right (responsive)
    let (info_pct, chat_pct) = detail_split(&chunks[1]);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([info_pct, chat_pct])
        .split(chunks[1]);

    // Info panel
    let info_active = app.focus != Focus::AgentChat;
    let ib = if info_active { t.border_active } else { t.border };

    let caps = if a.capabilities.is_empty() { "none".into() } else { a.capabilities.join(", ") };

    // OS-based ASCII art decoration
    let os_art = os_ascii_art(&a.os);

    let rows = vec![
        ("Host", a.host.clone(), t.text),
        ("Location", a.location.clone(), match a.location.as_str() {
            "Home" => t.loc_home, "SM" => t.loc_sm, "VPS" => t.loc_vps, "Mobile" => t.loc_mobile, _ => t.text,
        }),
        ("Status", a.status.to_string(), st_color),
        ("OS", a.os.clone(), t.text),
        ("Kernel", a.kernel.clone(), t.text),
        ("OC Version", a.oc_version.clone(), t.version),
        ("SSH User", a.ssh_user.clone(), t.text),
        ("Capabilities", caps, t.text),
        ("CPU", match a.cpu_pct { Some(p) => format!("{:.1}%", p), None => "—".into() },
            match a.cpu_pct { Some(p) if p > 90.0 => t.status_offline, Some(p) if p > 70.0 => t.status_busy, Some(_) => t.status_online, _ => t.text_dim }),
        ("RAM", match a.ram_pct { Some(p) => format!("{:.1}%", p), None => "—".into() },
            match a.ram_pct { Some(p) if p > 85.0 => t.status_offline, Some(p) if p > 70.0 => t.status_busy, Some(_) => t.status_online, _ => t.text_dim }),
        ("Disk", match a.disk_pct { Some(p) => format!("{:.0}%", p), None => "—".into() },
            match a.disk_pct { Some(p) if p > 90.0 => t.status_offline, Some(p) if p > 80.0 => t.status_busy, Some(_) => t.status_online, _ => t.text_dim }),
        ("Latency", match a.latency_ms { Some(ms) => format!("{}ms", ms), None => "—".into() },
            match a.latency_ms { Some(ms) if ms < 100 => t.status_online, Some(ms) if ms < 500 => t.status_busy, Some(_) => t.status_offline, _ => t.text_dim }),
        ("Tokens Today", format!("{}", a.token_burn), t.text),
        ("Last Seen", a.last_seen.clone(), t.text),
        ("Task", a.current_task.as_deref().unwrap_or("none").to_string(), t.text_dim),
    ];

    let mut info: Vec<Line> = rows.iter().map(|(l, v, c)| Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<14}", l), Style::default().fg(t.text_bold).bold()),
        Span::styled(v.clone(), Style::default().fg(*c)),
    ])).collect();

    // Append OS art decoration at the bottom of info panel
    info.push(Line::from(""));
    for art_line in os_art {
        info.push(Line::from(Span::styled(art_line.to_string(), Style::default().fg(t.text_dim))));
    }

    let detail = Paragraph::new(info).block(Block::default()
        .title(Span::styled(" ◆── Info ──◆ ", Style::default().fg(ib).bold()))
        .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(ib))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 1, 0)));
    if app.focus == Focus::Workspace {
        render_workspace(frame, app, body[0]);
    } else if app.focus == Focus::Services {
        render_services(frame, app, body[0]);
    } else {
        frame.render_widget(detail, body[0]);
    }

    // Store hit zones
    app.detail_info_area = body[0];
    app.detail_chat_area = body[1];

    // Agent chat
    render_chat_panel(frame, app, body[1], app.focus == Focus::AgentChat, true);

    // Footer
    render_footer(frame, app, chunks[2]);
}

fn render_diagnostics(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();
    // Center overlay
    let w = 60.min(area.width.saturating_sub(4));
    let h = (app.diag_steps.len() as u16 + 6).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let agent_name = if app.selected < app.agents.len() { &app.agents[app.selected].name } else { "?" };
    let title = format!(" {} Diagnostics — {} ", if app.diag_auto_fix { "🔧 Fix" } else { "🔍 Check" }, agent_name);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    // Deduplicate — only show latest status per label
    let mut seen = std::collections::HashMap::new();
    for step in &app.diag_steps {
        seen.insert(step.label.clone(), step.clone());
    }
    // Maintain order of first appearance
    let mut ordered_labels = Vec::new();
    for step in &app.diag_steps {
        if !ordered_labels.contains(&step.label) {
            ordered_labels.push(step.label.clone());
        }
    }

    for label in &ordered_labels {
        if label == "DONE" { continue; }
        if let Some(step) = seen.get(label) {
            if step.label.contains("Diagnosing") {
                lines.push(Line::from(Span::styled(format!("  {}", step.label), Style::default().fg(t.accent).bold())));
                lines.push(Line::from(""));
                continue;
            }
            let (icon, color) = match step.status {
                DiagStatus::Running => ("⏳", t.pending),
                DiagStatus::Pass => ("✓ ", t.status_online),
                DiagStatus::Fail => ("✗ ", t.status_offline),
                DiagStatus::Fixed => ("🔧", Color::Yellow),
                DiagStatus::Skipped => ("⊘ ", t.text_dim),
            };
            let detail = if step.detail.is_empty() { String::new() } else { format!(" — {}", step.detail) };
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                Span::styled(&step.label, Style::default().fg(t.text).bold()),
                Span::styled(detail, Style::default().fg(t.text_dim)),
            ]));
        }
    }

    // Show done status
    if let Some(done) = seen.get("DONE") {
        lines.push(Line::from(""));
        let total = ordered_labels.len() - 2; // minus header and DONE
        let passed = ordered_labels.iter().filter(|l| {
            *l != "DONE" && !seen.get(*l).map(|s| s.label.contains("Diagnosing")).unwrap_or(false)
                && seen.get(*l).map(|s| matches!(s.status, DiagStatus::Pass | DiagStatus::Fixed)).unwrap_or(false)
        }).count();
        let failed = total - passed;
        let summary = if failed == 0 {
            format!("  All {} checks passed ✓", total)
        } else {
            format!("  {}/{} passed, {} failed", passed, total, failed)
        };
        let color = if failed == 0 { t.status_online } else { t.status_offline };
        lines.push(Line::from(Span::styled(summary, Style::default().fg(color).bold())));
        lines.push(Line::from(Span::styled("  Press Esc to close", Style::default().fg(t.text_dim))));
    }

    let diag = Paragraph::new(lines)
        .block(Block::default()
            .title(Span::styled(title, Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL).border_type(BorderType::Double)
            .border_style(Style::default().fg(if app.diag_auto_fix { Color::Yellow } else { t.accent }))
            .style(Style::default().bg(Color::Rgb(15, 17, 22))));
    frame.render_widget(diag, popup);
}

fn render_services(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let split = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(40)])
        .split(area);

    // Left: service list
    let mut items: Vec<Line> = Vec::new();
    items.push(Line::from(Span::styled("  🔌 Services & Plugins", Style::default().fg(t.header_title).bold())));
    items.push(Line::from(""));

    if app.svc_loading {
        items.push(Line::from(Span::styled("  Loading config...", Style::default().fg(t.pending))));
    } else if app.svc_list.is_empty() {
        items.push(Line::from(Span::styled("  No config loaded", Style::default().fg(t.text_dim))));
    } else {
        for (i, svc) in app.svc_list.iter().enumerate() {
            let selected = i == app.svc_selected;
            let prefix = if selected { " ▸ " } else { "   " };
            let status_icon = if svc.name == "model" || svc.name == "gateway" {
                "◆"
            } else if svc.enabled { "●" } else { "○" };
            let status_color = if svc.enabled { t.status_online } else { t.text_dim };
            let name_style = if selected {
                Style::default().fg(Color::Black).bg(t.accent).bold()
            } else {
                Style::default().fg(if svc.enabled { t.text } else { t.text_dim })
            };
            items.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(t.accent)),
                Span::styled(format!("{} ", status_icon), Style::default().fg(status_color)),
                Span::styled(format!("{} ", svc.icon), Style::default()),
                Span::styled(format!("{:<16}", svc.name), name_style),
            ]));
        }
    }

    items.push(Line::from(""));
    items.push(Line::from(Span::styled("  ↑↓ select  Space toggle", Style::default().fg(t.text_dim))));
    items.push(Line::from(Span::styled("  Enter details  r reload", Style::default().fg(t.text_dim))));

    let list = Paragraph::new(items)
        .block(Block::default()
            .title(Span::styled(" Services ", Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL).border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(list, split[0]);

    // Right: service detail
    let detail_lines = if app.svc_selected < app.svc_list.len() {
        let svc = &app.svc_list[app.svc_selected];
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("  {} ", svc.icon), Style::default()),
                Span::styled(&svc.name, Style::default().fg(t.header_title).bold()),
                Span::raw("  "),
                Span::styled(
                    if svc.enabled { "● enabled" } else { "○ disabled" },
                    Style::default().fg(if svc.enabled { t.status_online } else { t.text_dim }),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Summary: ", Style::default().fg(t.text_bold).bold()),
                Span::styled(&svc.summary, Style::default().fg(t.text)),
            ]),
            Line::from(""),
        ];

        // Show raw config section if available
        if let Some(ref config) = app.svc_config {
            let section = if svc.name == "gateway" {
                config.get("gateway")
            } else if svc.name == "model" {
                config.get("agents")
            } else {
                // Show channel config if exists, otherwise plugin config
                config.get("channels").and_then(|c| c.get(&svc.name))
                    .or_else(|| config.get("plugins").and_then(|p| p.get("entries")).and_then(|e| e.get(&svc.name)))
            };

            if let Some(section) = section {
                lines.push(Line::from(Span::styled("  Configuration:", Style::default().fg(t.text_bold).bold())));
                lines.push(Line::from(""));
                let pretty = serde_json::to_string_pretty(section).unwrap_or_default();
                for line in pretty.lines() {
                    // Syntax highlight JSON
                    let styled = if line.contains(':') {
                        let parts: Vec<&str> = line.splitn(2, ':').collect();
                        Line::from(vec![
                            Span::styled(format!("  {}", parts[0]), Style::default().fg(t.accent)),
                            Span::styled(format!(":{}", parts.get(1).unwrap_or(&"")), Style::default().fg(t.text)),
                        ])
                    } else {
                        Line::from(Span::styled(format!("  {}", line), Style::default().fg(t.text_dim)))
                    };
                    lines.push(styled);
                }
            }
        }
        lines
    } else {
        vec![Line::from(Span::styled("  Select a service", Style::default().fg(t.text_dim)))]
    };

    let detail_title = if app.svc_selected < app.svc_list.len() {
        format!(" {} Detail ", app.svc_list[app.svc_selected].name)
    } else {
        " Detail ".to_string()
    };

    let detail = Paragraph::new(detail_lines)
        .scroll((app.svc_detail_scroll, 0))
        .block(Block::default()
            .title(Span::styled(detail_title, Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL).border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(detail, split[1]);
}

fn render_workspace(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;

    let split = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(40)])
        .split(area);

    // Left: file list + crons
    let mut items: Vec<Line> = Vec::new();
    items.push(Line::from(Span::styled("  📁 Agent Files", Style::default().fg(t.header_title).bold())));
    items.push(Line::from(""));

    for (i, f) in app.ws_files.iter().enumerate() {
        let selected = i == app.ws_selected;
        let prefix = if selected { " ▸ " } else { "   " };
        let status = if f.exists {
            let sz = f.size_bytes.map(|s| {
                if s > 1024 { format!(" {}K", s / 1024) } else { format!(" {}B", s) }
            }).unwrap_or_default();
            format!("✓{}", sz)
        } else {
            "✗ missing".to_string()
        };
        let name_style = if selected {
            Style::default().fg(Color::Black).bg(t.accent).bold()
        } else if f.exists {
            Style::default().fg(t.text)
        } else {
            Style::default().fg(t.text_dim)
        };
        items.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(t.accent)),
            Span::styled(format!("{} ", f.icon), Style::default()),
            Span::styled(format!("{:<18}", f.name), name_style),
            Span::styled(status, Style::default().fg(if f.exists { t.status_online } else { t.text_dim })),
        ]));
    }

    // Crons section
    if !app.ws_crons.is_empty() {
        items.push(Line::from(""));
        items.push(Line::from(Span::styled("  ⏰ Cron Jobs", Style::default().fg(t.header_title).bold())));
        items.push(Line::from(""));
        for cron in &app.ws_crons {
            let icon = if cron.enabled { "●" } else { "○" };
            let color = if cron.enabled { t.status_online } else { t.text_dim };
            let desc: String = if cron.description.len() > 22 {
                format!("{}…", &cron.description[..21])
            } else {
                cron.description.clone()
            };
            items.push(Line::from(vec![
                Span::styled(format!("   {} ", icon), Style::default().fg(color)),
                Span::styled(format!("{:<8}", cron.schedule), Style::default().fg(t.text_dim)),
                Span::styled(desc, Style::default().fg(t.text)),
            ]));
        }
    }

    if app.ws_loading {
        items.clear();
        items.push(Line::from(""));
        items.push(Line::from(Span::styled("  Loading workspace...", Style::default().fg(t.pending))));
    }

    // Keybind hints
    items.push(Line::from(""));
    items.push(Line::from(Span::styled("  ↑↓ select  Enter view", Style::default().fg(t.text_dim))));
    items.push(Line::from(Span::styled("  e edit  Tab→chat", Style::default().fg(t.text_dim))));

    let file_panel = Paragraph::new(items)
        .block(Block::default()
            .title(Span::styled(" Workspace ", Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL).border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(file_panel, split[0]);

    // Right: file content viewer
    let content_text = if let Some(ref content) = app.ws_content {
        let lines: Vec<Line> = content.lines().enumerate().map(|(i, line)| {
            Line::from(vec![
                Span::styled(format!("{:>4} │ ", i + 1), Style::default().fg(t.text_dim)),
                Span::styled(line.to_string(), Style::default().fg(t.text)),
            ])
        }).collect();
        lines
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled("  Select a file and press Enter to view", Style::default().fg(t.text_dim))),
            Line::from(""),
            Line::from(Span::styled("  Press 'e' to edit the selected file", Style::default().fg(t.text_dim))),
        ]
    };

    let file_title = if app.ws_selected < app.ws_files.len() {
        format!(" {} {} ", app.ws_files[app.ws_selected].icon, app.ws_files[app.ws_selected].name)
    } else {
        " File Viewer ".to_string()
    };

    let viewer = Paragraph::new(content_text)
        .scroll((app.ws_content_scroll, 0))
        .block(Block::default()
            .title(Span::styled(file_title, Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL).border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(viewer, split[1]);
}

fn render_vpn_status(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let online = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🔒 VPN MESH STATUS", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("{}/{} nodes reachable", online, app.agents.len()), Style::default().fg(t.status_online)),
        Span::raw("    "),
        Span::styled("Headscale (self-hosted)", Style::default().fg(t.text_dim)),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Double)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(header, outer[0]);

    // Node table
    let hcells = ["  ", "Agent", "Tailscale IP", "Status", "Location", "OC Version"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.agents.iter().map(|a| {
        let st_color = match a.status {
            AgentStatus::Online => t.status_online,
            AgentStatus::Busy => t.status_busy,
            AgentStatus::Offline => t.status_offline,
            _ => t.text_dim,
        };
        let loc_c = match a.location.as_str() {
            "Home" => t.loc_home, "SM" => t.loc_sm, "VPS" => t.loc_vps, "Mobile" => t.loc_mobile, _ => t.text,
        };
        Row::new(vec![
            Cell::from(format!(" {}", a.emoji)),
            Cell::from(a.name.clone()).style(Style::default().fg(t.text_bold).bold()),
            Cell::from(a.host.clone()).style(Style::default().fg(t.accent2)),
            Cell::from(format!("{}", a.status)).style(Style::default().fg(st_color)),
            Cell::from(a.location.clone()).style(Style::default().fg(loc_c)),
            Cell::from(a.oc_version.clone()).style(Style::default().fg(t.version)),
        ]).style(Style::default().bg(app.bg_density.bg())).height(1)
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(4), Constraint::Length(16), Constraint::Length(15),
        Constraint::Length(14), Constraint::Length(9), Constraint::Min(12),
    ]).header(hrow)
    .block(Block::default().title(Span::styled(" ◆── Mesh Nodes ──◆ ", Style::default().fg(t.border_active).bold()))
        .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(t.border_active))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, outer[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("Esc=back │ Headscale at vpn.example.com │ v=VPN │ q=quit", Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, outer[2]);
}


fn render_task_board(frame: &mut Frame, app: &App) {
    let filter_label = app.task_filter_agent.as_ref()
        .map(|a| format!(" ({})", a)).unwrap_or_default();
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3), Constraint::Length(3)])
        .split(frame.area());

    // BG
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    // Header
    let queued = app.tasks.iter().filter(|t| t.status == "queued").count();
    let running = app.tasks.iter().filter(|t| t.status == "running" || t.status == "assigned").count();
    let done = app.tasks.iter().filter(|t| t.status == "completed").count();

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("📋 TASK BOARD", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("{} queued", queued), Style::default().fg(t.sender_self)),
        Span::raw("  "),
        Span::styled(format!("{} active", running), Style::default().fg(t.status_busy)),
        Span::raw("  "),
        Span::styled(format!("{} done", done), Style::default().fg(t.status_online)),
        Span::raw("    "),
        Span::styled(format!("{} total", app.tasks.len()), Style::default().fg(t.text_dim)),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Double)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(header, outer[0]);

    // Task body — split into list (left) and detail (right)
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[1]);

    // Task list
    let hcells = ["  #", "P", "Status", "Agent", "Description"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.tasks.iter().enumerate().map(|(i, task)| {
        let sel = i == app.task_selected;
        let bg = if sel { t.selected_bg } else if i % 2 == 1 { ratatui::style::Color::Rgb(20, 22, 28) } else { app.bg_density.bg() };
        let is_multi = app.multi_selected.contains(&i);
        let cursor = if sel && is_multi { "▶✓" } else if sel { "▶ " } else if is_multi { " ✓" } else { "  " };

        let st_color = match task.status.as_str() {
            "queued" => t.pending,
            "assigned" => t.sender_self,
            "running" => t.status_busy,
            "completed" => t.status_online,
            "failed" => t.status_offline,
            _ => t.text_dim,
        };

        let st_icon = match task.status.as_str() {
            "queued" => "⏳",
            "assigned" => "📨",
            "running" => "🔄",
            "completed" => "✅",
            "failed" => "❌",
            _ => "?",
        };

        let pri_color = match task.priority {
            1..=3 => t.status_offline,
            4..=6 => t.status_busy,
            _ => t.status_online,
        };

        // Priority indicator: 🔥 for P9-10, ▶ for P7-8, · for <7
        let pri_indicator = match task.priority {
            9..=10 => "🔥",
            7..=8 => "▶",
            _ => "·",
        };

        let desc: String = task.description.chars().take(30).collect();

        Row::new(vec![
            Cell::from(format!("{}{}", cursor, task.id)),
            Cell::from(format!("{} {}", pri_indicator, task.priority)).style(Style::default().fg(pri_color).bold()),
            Cell::from(format!("{} {}", st_icon, task.status)).style(Style::default().fg(st_color)),
            Cell::from(task.assigned_agent.as_deref().unwrap_or("—").to_string()).style(Style::default().fg(t.accent2)),
            Cell::from(desc).style(Style::default().fg(t.text)),
        ]).style(Style::default().bg(bg)).height(1)
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(5), Constraint::Length(5), Constraint::Length(14),
        Constraint::Length(14), Constraint::Min(15),
    ]).header(hrow)
    .block(Block::default().title(Span::styled(format!(" ◆── Tasks{} ──◆ ", filter_label), Style::default().fg(t.border_active).bold()))
        .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(t.border_active))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, body[0]);

    // Task detail (right side)
    let detail_lines = if let Some(task) = app.tasks.get(app.task_selected) {
        let st_color = match task.status.as_str() {
            "completed" => t.status_online, "failed" => t.status_offline,
            "running" => t.status_busy, _ => t.text,
        };
        let pri_indicator = match task.priority {
            9..=10 => "🔥",
            7..=8 => "▶",
            _ => "·",
        };
        vec![
            Line::from(vec![
                Span::styled("  ID          ", Style::default().fg(t.text_bold).bold()),
                Span::styled(format!("#{}", task.id), Style::default().fg(t.accent)),
            ]),
            Line::from(vec![
                Span::styled("  Priority    ", Style::default().fg(t.text_bold).bold()),
                Span::styled(format!("{} {}", pri_indicator, task.priority), Style::default().fg(t.text)),
            ]),
            Line::from(vec![
                Span::styled("  Status      ", Style::default().fg(t.text_bold).bold()),
                Span::styled(&task.status, Style::default().fg(st_color)),
            ]),
            Line::from(vec![
                Span::styled("  Agent       ", Style::default().fg(t.text_bold).bold()),
                Span::styled(task.assigned_agent.as_deref().unwrap_or("unassigned"), Style::default().fg(t.accent2)),
            ]),
            Line::from(vec![
                Span::styled("  Created     ", Style::default().fg(t.text_bold).bold()),
                Span::styled(format!("{} by {}", task.created_at, task.created_by), Style::default().fg(t.text_dim)),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Description:", Style::default().fg(t.text_bold).bold())),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(&task.description, Style::default().fg(t.text)),
            ]),
            Line::from(""),
            if let Some(result) = &task.result {
                Line::from(vec![
                    Span::styled("  Result: ", Style::default().fg(t.text_bold).bold()),
                    Span::styled(result.as_str(), Style::default().fg(t.response)),
                ])
            } else {
                Line::from(Span::styled("  No result yet", Style::default().fg(t.text_dim)))
            },
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled("  No tasks yet", Style::default().fg(t.text_dim))),
            Line::from(Span::styled("  Press 'n' to create one", Style::default().fg(t.text_dim))),
        ]
    };

    let detail = Paragraph::new(detail_lines)
        .block(Block::default().title(Span::styled(" ◆── Detail ──◆ ", Style::default().fg(t.border).bold()))
            .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(0, 1, 1, 0)));
    frame.render_widget(detail, body[1]);

    // New task input
    let input_active = app.task_input_active;
    let ib = if input_active { t.border_active } else { t.border };
    let prompt = if input_active { " new task description ⏎ " } else { " n=new task  d=done  Esc=back " };
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(t.accent)),
        Span::styled(&app.task_input, Style::default().fg(t.text)),
        if input_active { Span::styled("▌", Style::default().fg(t.accent)) } else { Span::raw("") },
    ])).block(Block::default().title(prompt)
        .borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(ib))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(input, outer[2]);

    // Footer
    let footer_msg = format!("v0.9 │ t=tasks │ n=new │ d=done │ j/k=navigate │ Esc=back │ {}/{}",
        app.theme_name.label(), app.bg_density.label());
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(footer_msg, Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, outer[3]);
}


fn render_alerts(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let outer = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let crits = app.alerts.iter().filter(|a| a.severity == AlertSeverity::Critical).count();
    let warns = app.alerts.iter().filter(|a| a.severity == AlertSeverity::Warning).count();
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🔔 ALERTS", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("🔴 {}", crits), Style::default().fg(t.status_offline)),
        Span::raw("  "),
        Span::styled(format!("🟡 {}", warns), Style::default().fg(t.status_busy)),
        Span::raw("  "),
        Span::styled(format!("{} total", app.alerts.len()), Style::default().fg(t.text_dim)),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Double)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(header, outer[0]);

    let lines: Vec<Line> = if app.alerts.is_empty() {
        vec![Line::from(""), Line::from(Span::styled("  No alerts — all systems nominal ✅", Style::default().fg(t.status_online)))]
    } else {
        app.alerts.iter().rev().map(|a| {
            let sev_color = match a.severity {
                AlertSeverity::Critical => t.status_offline,
                AlertSeverity::Warning => t.status_busy,
                AlertSeverity::Info => t.accent,
            };
            Line::from(vec![
                Span::styled(format!("  {} ", a.time), Style::default().fg(t.text_dim)),
                Span::styled(a.severity.icon(), Style::default()),
                Span::raw(" "),
                Span::styled(format!("{} ", a.emoji), Style::default()),
                Span::styled(&a.message, Style::default().fg(sev_color)),
            ])
        }).collect()
    };

    let alerts = Paragraph::new(lines)
        .block(Block::default().title(Span::styled(" ◆── Alert History ──◆ ", Style::default().fg(t.border_active).bold()))
            .borders(Borders::ALL).border_type(t.border_type).border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 1, 0)));
    frame.render_widget(alerts, outer[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("Esc=back │ w=alerts │ q=quit", Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, outer[2]);
}

fn render_help(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let sections = vec![
        ("", ""),
        ("DASHBOARD", ""),
        ("  Tab", "Switch focus: Fleet ↔ Chat"),
        ("  ↑↓ / j k", "Navigate fleet list"),
        ("  Enter", "Open agent detail"),
        ("  r", "Refresh all agents (SSH)"),
        ("  s", "Sort: name → status → location → version"),
        ("  t", "Task board"),
        ("  v", "VPN mesh status"),
        ("  w", "Alerts & warnings"),
        ("  Space", "Toggle agent selection"),
        ("  A (Shift)", "Select all agents"),
        ("  N (Shift)", "Clear selection"),
        ("  a", "New agent wizard"),
        ("  /", "Fleet command (runs on all agents)"),
        ("  g", "Restart gateway (selected)"),
        ("  G (Shift)", "Investigate gateway (selected)"),
        ("  o", "OpenClaw version audit"),
        ("  u", "Bulk update OpenClaw"),
        ("  g", "Restart gateway (selected agent)"),
        ("  c", "Cycle color theme"),
        ("  b", "Cycle background density"),
        ("  q", "Quit"),
        ("", ""),
        ("AGENT DETAIL", ""),
        ("  e", "View agent config (openclaw.json)"),
        ("  Tab", "Switch: Info ↔ Chat"),
        ("  Enter", "Send direct message"),
        ("  Esc", "Back to dashboard"),
        ("", ""),
        ("TASK BOARD", ""),
        ("  j / k", "Navigate tasks"),
        ("  n", "Create new task"),
        ("  d", "Mark done"),
        ("  Esc", "Back"),
        ("", ""),
        ("MOUSE", ""),
        ("  Click", "Focus panel / select agent"),
        ("  Scroll", "Scroll chat panels"),
        ("", ""),
        ("THEMES (10)", "standard noir paper 1977 2077 matrix sunset arctic ocean ember"),
        ("BACKGROUNDS", "dark medium light white terminal"),
    ];

    let lines: Vec<Line> = sections.iter().map(|(l, r)| {
        if r.is_empty() && !l.is_empty() && !l.starts_with(' ') {
            Line::from(Span::styled(format!("  {}", l), Style::default().fg(t.accent).bold()))
        } else {
            Line::from(vec![
                Span::styled(format!("  {:<14}", l), Style::default().fg(t.sender_self)),
                Span::styled(r.to_string(), Style::default().fg(t.text)),
            ])
        }
    }).collect();

    let help = Paragraph::new(lines).block(Block::default()
        .title(Span::styled(" Help — press any key to close ", Style::default().fg(t.accent).bold()))
        .borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(2, 2, 1, 1)));
    frame.render_widget(help, frame.area());
}


fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;

    // Breadcrumb
    let crumb = match app.screen {
        Screen::Dashboard => "Dashboard".to_string(),
        Screen::AgentDetail => {
            let name = if app.selected < app.agents.len() { &app.agents[app.selected].name } else { "?" };
            let tab = match app.focus {
                Focus::AgentChat => "Chat",
                Focus::Workspace => "Files",
                Focus::Services => "Services",
                _ => "Info",
            };
            format!("Dashboard › {} › {}", name, tab)
        }
        Screen::TaskBoard => {
            if let Some(ref agent) = app.task_filter_agent {
                format!("Dashboard › {} › Tasks", agent)
            } else {
                "Dashboard › Tasks".to_string()
            }
        }
        Screen::Help => "Help".to_string(),
        _ => "Dashboard".to_string(),
    };

    // Build styled key hints (key highlighted, label dim)
    let keys: Vec<(&str, &str)> = match app.screen {
        Screen::Dashboard if app.filter_active => vec![("type","filter"),("↑↓","navigate"),("⏎","apply"),("Esc","cancel")],
        Screen::Dashboard => match app.focus {
            Focus::Chat => vec![("Tab","fleet"),("⏎","send"),("@","target"),("Esc","back")],
            Focus::Command => vec![("⏎","run"),("Esc","cancel")],
            _ => vec![("⏎","open"),("d","check"),("D","fix"),("t","tasks"),("f","filter"),("s","sort"),("r","refresh"),("?","help"),("q","quit")],
        },
        Screen::AgentDetail => match app.focus {
            Focus::AgentChat => vec![("⏎","send"),("@","tag"),("Tab","next"),("Esc","info"),("1-5","tabs")],
            Focus::Workspace => vec![("⏎","view"),("e","edit"),("r","reload"),("Esc","info"),("1-5","tabs")],
            Focus::Services => vec![("␣","toggle"),("r","reload"),("Esc","info"),("1-5","tabs")],
            _ => vec![("⏎","detail"),("d","check"),("D","fix"),("w","files"),("t","tasks"),("5","svc"),("Tab","chat"),("Esc","back")],
        },
        Screen::TaskBoard => if app.task_filter_agent.is_some() {
            vec![("n","new"),("d","done"),("c","clear"),("1-5","tabs"),("Esc","back")]
        } else {
            vec![("n","new"),("d","done"),("Esc","back")]
        },
        Screen::Help => vec![("Esc","back"),("q","quit")],
        _ => vec![("Esc","back")],
    };

    // Toast (auto-dismiss after 4s)
    let show_toast = app.toast_at.map(|t| t.elapsed() < Duration::from_secs(4)).unwrap_or(false);
    let toast_text = if show_toast { app.toast_message.as_deref().unwrap_or("") } else { "" };

    // Build left side (breadcrumb)
    let mut left_spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(&crumb, Style::default().fg(t.accent).bold()),
    ];

    // Build right side
    let mut right_spans: Vec<Span> = Vec::new();
    if !toast_text.is_empty() {
        right_spans.push(Span::styled(toast_text, Style::default().fg(Color::Yellow).bold()));
    } else {
        for (i, (key, label)) in keys.iter().enumerate() {
            if i > 0 { right_spans.push(Span::styled(" ", Style::default().fg(t.text_dim))); }
            right_spans.push(Span::styled(format!(" {} ", key), Style::default().fg(Color::Black).bg(t.accent).bold()));
            right_spans.push(Span::styled(format!("{}", label), Style::default().fg(t.text_dim)));
        }
    }
    right_spans.push(Span::raw("  "));

    // Calculate padding between left and right
    let left_len: usize = crumb.len() + 2;
    let right_len: usize = if !toast_text.is_empty() {
        toast_text.len() + 2
    } else {
        keys.iter().map(|(k, l)| k.len() + l.len() + 3).sum::<usize>() + 2
    };
    let pad = (area.width as usize).saturating_sub(left_len + right_len + 4);
    left_spans.push(Span::raw(" ".repeat(pad)));

    let mut all_spans = left_spans;
    all_spans.extend(right_spans);

    let footer = Paragraph::new(Line::from(all_spans))
        .block(Block::default().borders(Borders::ALL).border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, area);
}

// ---- Main ----

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = cli::Cli::parse();

    // Load config (config.toml → .env → defaults)
    let sam_config = cli::SamConfig::load(args.config.as_ref());
    sam_config.apply_to_env();

    // .env as fallback
    if dotenvy::dotenv().is_err() {
        if let Ok(home) = std::env::var("HOME") {
            let _ = dotenvy::from_path(std::path::Path::new(&home).join(".config/sam/.env"));
        }
    }

    // Handle subcommands
    match args.command {
        Some(cli::Commands::Setup) => { return cli::run_setup().map_err(|e| e.into()); }
        Some(cli::Commands::Status) => { return cli::print_status().await.map_err(|e| e.into()); }
        Some(cli::Commands::Chat { agent, message }) => {
            let msg = message.join(" ");
            return cli::send_chat(&agent, &msg).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Doctor { fix, agent }) => {
            return cli::run_doctor(fix, agent.as_deref()).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Init { db_host, db_port, db_user, db_pass, db_name, self_ip }) => {
            return cli::run_init(db_host.as_deref(), db_port, db_user.as_deref(), db_pass.as_deref(), db_name.as_deref(), self_ip.as_deref()).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Deploy { target, file, source }) => {
            return cli::run_deploy(&target, &file, source.as_deref()).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Onboard { host, user, name }) => {
            return cli::run_onboard(&host, &user, name.as_deref()).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Version) => {
            println!("sam v{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        None => {} // Launch TUI
    }

    let fleet_config = match config::load_fleet_config() {
        Ok(c) => c,
        Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); }
    };

    // Install panic hook that restores terminal before printing panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(crossterm::event::DisableMouseCapture);
        let _ = stdout().execute(LeaveAlternateScreen);
        // Write crash log
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
            .open("/tmp/sam-crash.log") {
            use std::io::Write;
            let _ = writeln!(f, "[{}] PANIC: {}", now_str(), info);
        }
        original_hook(info);
    }));

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(crossterm::event::EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = App::new(fleet_config).await;
    app.update_status_bar();

    loop {
        // Drain background probe results (non-blocking)
        let updates = app.drain_refresh_results();
        if !updates.is_empty() {
            // Write to DB in background
            if let Some(pool) = &app.db_pool {
                for (idx, _status, _os, _kern, _oc, lat) in &updates {
                    let a = &app.agents[*idx];
                    let p = pool.clone();
                    let (name, st, os, kern, oc, latency) = (
                        a.db_name.clone(), a.status.to_db_str().to_string(),
                        if a.os.is_empty() { None } else { Some(a.os.clone()) },
                        if a.kernel.is_empty() { None } else { Some(a.kernel.clone()) },
                        if a.oc_version.is_empty() { None } else { Some(a.oc_version.clone()) },
                        *lat,
                    );
                    tokio::spawn(async move {
                        let _ = db::update_agent_status_full(&p, &name, &st,
                            os.as_deref(), kern.as_deref(), oc.as_deref(), latency).await;
                    });
                }
            }
            app.check_alerts();
            app.update_status_bar();
        }

        terminal.draw(|f| {
            if app.show_splash {
                render_splash(f, &app);
            } else {
            match app.screen {
                Screen::Dashboard => render_dashboard(f, &mut app),
                Screen::AgentDetail => render_detail(f, &mut app),
                Screen::TaskBoard => render_task_board(f, &app),
                Screen::VpnStatus => render_vpn_status(f, &app),
                Screen::Alerts => render_alerts(f, &app),
                Screen::Help => render_help(f, &app),
                Screen::SpawnManager => render_help(f, &app),
            }
            // Diagnostic overlay (renders on top of everything)
            if app.diag_active {
                render_diagnostics(f, &app);
            }

            // Config viewer overlay
            if let Some(config) = &app.config_text {
                let t = &app.theme;
                let area = f.area();
                let w = (area.width as f32 * 0.7) as u16;
                let h = (area.height as f32 * 0.8) as u16;
                let x = (area.width - w) / 2;
                let y = (area.height - h) / 2;
                let rect = Rect::new(x, y, w, h);
                let clear = Block::default().style(Style::default().bg(app.bg_density.bg()));
                f.render_widget(clear, rect);
                let lines: Vec<Line> = config.lines().map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(t.text)))).collect();
                let p = Paragraph::new(lines).scroll((app.config_scroll, 0))
                    .block(Block::default()
                        .title(Span::styled(" openclaw.json — Esc to close ", Style::default().fg(t.accent).bold()))
                        .borders(Borders::ALL).border_type(t.border_type)
                        .border_style(Style::default().fg(t.accent))
                        .style(Style::default().bg(app.bg_density.bg()))
                        .padding(Padding::new(1, 1, 1, 0)));
                f.render_widget(p, rect);
            }
            if app.wizard.active {
                wizard::render_wizard(f, &app.wizard, &app.theme, app.bg_density.bg());
            }
            } // close else for show_splash
        })?;

        if event::poll(Duration::from_millis(INPUT_POLL_MS))? {
            let ev = event::read()?;

            // Splash dismiss
            if app.show_splash {
                if let Event::Key(_) = &ev { app.show_splash = false; }
                continue;
            }

            // Mouse events
            if let Event::Mouse(mouse) = &ev {
                if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                    let (mx, my) = (mouse.column, mouse.row);
                    match app.screen {
                        Screen::Dashboard => {
                            // Click on fleet panel
                            if mx >= app.fleet_area.x && mx < app.fleet_area.x + app.fleet_area.width
                                && my >= app.fleet_area.y && my < app.fleet_area.y + app.fleet_area.height
                            {
                                app.focus = Focus::Fleet;
                                // Calculate which agent row was clicked
                                if my > app.fleet_row_start_y && app.fleet_row_start_y > 0 {
                                    let row = (my - app.fleet_row_start_y - 1) as usize; // -1 for header
                                    if row < app.agents.len() {
                                        app.selected = row;
                                    }
                                }
                            }
                            // Click on chat panel
                            else if mx >= app.chat_area.x && mx < app.chat_area.x + app.chat_area.width
                                && my >= app.chat_area.y && my < app.chat_area.y + app.chat_area.height
                            {
                                app.focus = Focus::Chat;
                            }
                        }
                        Screen::AgentDetail => {
                            if mx >= app.detail_info_area.x && mx < app.detail_info_area.x + app.detail_info_area.width
                                && my >= app.detail_info_area.y && my < app.detail_info_area.y + app.detail_info_area.height
                            {
                                app.focus = Focus::Fleet;
                            }
                            else if mx >= app.detail_chat_area.x && mx < app.detail_chat_area.x + app.detail_chat_area.width
                                && my >= app.detail_chat_area.y && my < app.detail_chat_area.y + app.detail_chat_area.height
                            {
                                app.focus = Focus::AgentChat;
                            }
                        }
                        _ => {}
                    }
                }

                // Scroll wheel in chat
                if let MouseEventKind::ScrollUp = mouse.kind {
                    match app.focus {
                        Focus::Chat => app.chat_scroll = app.chat_scroll.saturating_add(3),
                        Focus::AgentChat => app.agent_chat_scroll = app.agent_chat_scroll.saturating_add(3),
                        _ => {}
                    }
                }
                if let MouseEventKind::ScrollDown = mouse.kind {
                    match app.focus {
                        Focus::Chat => app.chat_scroll = app.chat_scroll.saturating_sub(3),
                        Focus::AgentChat => app.agent_chat_scroll = app.agent_chat_scroll.saturating_sub(3),
                        _ => {}
                    }
                }
            }

            if let Event::Key(key) = ev {
                if key.kind == KeyEventKind::Press {
                    // Wizard overlay intercepts all input when active
                    if app.wizard.active {
                        match key.code {
                            KeyCode::Esc => {
                                if app.wizard.go_back() {
                                    app.wizard.active = false;
                                }
                            }
                            KeyCode::Enter => {
                                let ready = app.wizard.advance();
                                if ready {
                                    // Create the agent
                                    if let Some(pool) = &app.db_pool {
                                        let w = &app.wizard;
                                        let caps = format!(r#"["{}"]"#, w.location_str().to_lowercase());
                                        let _ = pool.get_conn().await.map(|mut conn| {
                                            let name = w.agent_name.clone();
                                            let host = w.host.clone();
                                            let _loc = w.location_str().to_string();
                                            let _ssh = w.ssh_user.clone();
                                            let _emoji = w.emoji.clone();
                                            let _display = w.display_name.clone();
                                            tokio::spawn(async move {
                                                use mysql_async::prelude::*;
                                                let _ = conn.exec_drop(
                                                    "INSERT IGNORE INTO mc_fleet_status (agent_name, tailscale_ip, status, capabilities) VALUES (?, ?, 'offline', ?)",
                                                    (&name, &host, &caps),
                                                ).await;
                                            });
                                        });
                                    }
                                    // Add to fleet config in memory
                                    app.fleet_config.push(config::AgentConfig {
                                        name: app.wizard.agent_name.clone(),
                                        display: Some(app.wizard.display_name.clone()),
                                        emoji: Some(app.wizard.emoji.clone()),
                                        location: Some(app.wizard.location_str().to_string()),
                                        ssh_user: Some(app.wizard.ssh_user.clone()),
                                    });
                                    // Add to agents vec
                                    app.agents.push(Agent {
                                        name: app.wizard.display_name.clone(),
                                        db_name: app.wizard.agent_name.clone(),
                                        emoji: app.wizard.emoji.clone(),
                                        host: app.wizard.host.clone(),
                                        location: app.wizard.location_str().to_string(),
                                        status: AgentStatus::Unknown,
                                        os: String::new(), kernel: String::new(),
                                        oc_version: String::new(), last_seen: String::new(),
                                        current_task: None,
                                        ssh_user: app.wizard.ssh_user.clone(),
                                        capabilities: vec![],
                                        token_burn: 0,
                                        latency_ms: None,
                                        cpu_pct: None, ram_pct: None, disk_pct: None,
                                        gateway_port: 18789,
                                        gateway_token: None,
                                        uptime_seconds: 0,
                                        activity: "new".into(), context_pct: None,
                                        last_probe_at: None,
                                    });
                                    app.wizard.active = false;
                                    app.status_message = format!("✅ Agent '{}' created", app.wizard.agent_name);
                                }
                            }
                            KeyCode::Tab => {
                                // Test SSH on confirm step
                                if app.wizard.step == wizard::WizardStep::Confirm {
                                    let host = app.wizard.host.clone();
                                    let user = app.wizard.ssh_user.clone();
                                    app.wizard.testing_ssh = true;
                                    app.wizard.ssh_result = Some("Testing...".into());
                                    let result = tokio::process::Command::new("ssh")
                                        .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                            &format!("{}@{}", user, host), "hostname && openclaw --version 2>/dev/null || echo 'OC not found'"])
                                        .output().await;
                                    app.wizard.testing_ssh = false;
                                    match result {
                                        Ok(o) if o.status.success() => {
                                            app.wizard.ssh_result = Some(format!("✅ {}", String::from_utf8_lossy(&o.stdout).trim()));
                                        }
                                        Ok(o) => {
                                            app.wizard.ssh_result = Some(format!("❌ {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(60).collect::<String>()));
                                        }
                                        Err(e) => {
                                            app.wizard.ssh_result = Some(format!("❌ {}", e));
                                        }
                                    }
                                } else {
                                    // Tab = skip/advance
                                    app.wizard.advance();
                                }
                            }
                            KeyCode::Backspace => app.wizard.pop_char(),
                            KeyCode::Char(ch) => app.wizard.push_char(ch),
                            _ => {}
                        }
                    } else {
                    // Diagnostic overlay intercepts all keys when active
                    if app.diag_active {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.diag_active = false;
                                app.diag_steps.clear();
                                app.start_refresh(); // re-probe after fix
                            }
                            _ => {}
                        }
                    } else {
                    match app.screen {
                        Screen::SpawnManager => { if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc { app.screen = Screen::Dashboard; } },
                    Screen::Help => { app.screen = Screen::Dashboard; }
                        Screen::AgentDetail if app.config_text.is_some() => match key.code {
                            KeyCode::Esc => { app.config_text = None; }
                            KeyCode::PageUp | KeyCode::Up => { app.config_scroll = app.config_scroll.saturating_add(3); }
                            KeyCode::PageDown | KeyCode::Down => { app.config_scroll = app.config_scroll.saturating_sub(3); }
                            _ => { app.config_text = None; }
                        },
                        Screen::AgentDetail => match app.focus {
                            Focus::Services => match key.code {
                                KeyCode::Esc => app.focus = Focus::Fleet,
                                KeyCode::Tab => app.focus = Focus::Fleet,
                                KeyCode::Char('1') => app.focus = Focus::Fleet,
                                KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                KeyCode::Char('3') => { app.focus = Focus::Workspace; app.start_workspace_load(); }
                                KeyCode::Char('4') | KeyCode::Char('t') => {
                                    app.task_filter_agent = Some(app.agents[app.selected].db_name.clone());
                                    app.screen = Screen::TaskBoard;
                                    app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                }
                                KeyCode::Char('5') => { app.focus = Focus::Services; app.start_services_load(); }
                                KeyCode::Char('5') => {} // already here
                                KeyCode::Up => { if app.svc_selected > 0 { app.svc_selected -= 1; app.svc_detail_scroll = 0; } }
                                KeyCode::Down => { if app.svc_selected < app.svc_list.len().saturating_sub(1) { app.svc_selected += 1; app.svc_detail_scroll = 0; } }
                                KeyCode::Char(' ') => app.toggle_service(),
                                KeyCode::Char('r') => app.start_services_load(),
                                KeyCode::PageUp => app.svc_detail_scroll = app.svc_detail_scroll.saturating_add(5),
                                KeyCode::PageDown => app.svc_detail_scroll = app.svc_detail_scroll.saturating_sub(5),
                                KeyCode::Char('q') => app.should_quit = true,
                                _ => {}
                            },
                            Focus::Workspace => match key.code {
                                KeyCode::Esc => app.focus = Focus::Fleet,
                                KeyCode::Tab => app.focus = Focus::Fleet,
                                KeyCode::Char('1') => app.focus = Focus::Fleet,
                                KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                KeyCode::Char('3') => {} // already here
                                KeyCode::Char('4') | KeyCode::Char('t') => {
                                    app.task_filter_agent = Some(app.agents[app.selected].db_name.clone());
                                    app.screen = Screen::TaskBoard;
                                    app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                }
                                KeyCode::Char('5') => { app.focus = Focus::Services; app.start_services_load(); }
                                KeyCode::Up => { if app.ws_selected > 0 { app.ws_selected -= 1; } }
                                KeyCode::Down => { if app.ws_selected < app.ws_files.len().saturating_sub(1) { app.ws_selected += 1; } }
                                KeyCode::Enter => app.start_file_load(),
                                KeyCode::Char('e') => {
                                    if let Some(ref c) = app.ws_content {
                                        app.ws_edit_buffer = c.clone();
                                        app.ws_editing = true;
                                    } else {
                                        app.start_file_load();
                                    }
                                }
                                KeyCode::Char('r') => app.start_workspace_load(),
                                KeyCode::PageUp => app.ws_content_scroll = app.ws_content_scroll.saturating_add(5),
                                KeyCode::PageDown => app.ws_content_scroll = app.ws_content_scroll.saturating_sub(5),
                                KeyCode::Char('q') => app.should_quit = true,
                                _ => {}
                            },
                            Focus::AgentChat => if app.ac_visible {
                                match key.code {
                                    KeyCode::Up => { if app.ac_selected > 0 { app.ac_selected -= 1; } else { app.ac_selected = app.ac_matches.len().saturating_sub(1); } }
                                    KeyCode::Down => { app.ac_selected = (app.ac_selected + 1) % app.ac_matches.len().max(1); }
                                    KeyCode::Tab | KeyCode::Enter => app.accept_autocomplete(),
                                    KeyCode::Esc => { app.ac_visible = false; }
                                    KeyCode::Backspace => { app.agent_chat_input.pop(); app.update_autocomplete(); }
                                    KeyCode::Char(c) => { app.agent_chat_input.push(c); app.update_autocomplete(); }
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc => app.focus = Focus::Fleet,
                                    KeyCode::Tab => { app.focus = Focus::Workspace; app.start_workspace_load(); }
                                    KeyCode::Char('1') if app.agent_chat_input.is_empty() => app.focus = Focus::Fleet,
                                    KeyCode::Char('3') if app.agent_chat_input.is_empty() => { app.focus = Focus::Workspace; app.start_workspace_load(); }
                                    KeyCode::Char('4') if app.agent_chat_input.is_empty() => {
                                        app.task_filter_agent = Some(app.agents[app.selected].db_name.clone());
                                        app.screen = Screen::TaskBoard;
                                        app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                    }
                                    KeyCode::Char('5') if app.agent_chat_input.is_empty() => { app.focus = Focus::Services; app.start_services_load(); }
                                    KeyCode::Enter => app.send_agent_message().await,
                                    KeyCode::Backspace => { app.agent_chat_input.pop(); app.update_autocomplete(); }
                                    KeyCode::Char(c) => { app.agent_chat_input.push(c); app.update_autocomplete(); }
                                    KeyCode::PageUp => app.agent_chat_scroll = app.agent_chat_scroll.saturating_add(5),
                                    KeyCode::PageDown => app.agent_chat_scroll = app.agent_chat_scroll.saturating_sub(5),
                                    _ => {}
                                }
                            },
                            _ => match key.code {
                                KeyCode::Esc => { app.screen = Screen::Dashboard; app.focus = Focus::Fleet; }
                                KeyCode::Tab => app.focus = Focus::AgentChat,
                                KeyCode::Char('1') => app.focus = Focus::Fleet,
                                KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                KeyCode::Char('3') => { app.focus = Focus::Workspace; app.start_workspace_load(); }
                                KeyCode::Char('w') => { app.focus = Focus::Workspace; app.start_workspace_load(); }
                                KeyCode::Char('d') => app.start_diagnostics(false),
                                KeyCode::Char('D') => app.start_diagnostics(true),
                                KeyCode::Char('4') | KeyCode::Char('t') => {
                                    app.task_filter_agent = Some(app.agents[app.selected].db_name.clone());
                                    app.screen = Screen::TaskBoard;
                                    app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                }
                                KeyCode::Char('q') => app.should_quit = true,
                                KeyCode::Char('r') => app.start_refresh(),
                                KeyCode::Char('d') => app.start_diagnostics(false),
                                KeyCode::Char('D') => app.start_diagnostics(true),
                                KeyCode::Char('b') => app.cycle_bg(),
                                KeyCode::Char('e') => {
                                    // Fetch remote config (non-blocking)
                                    if let Some(agent) = app.agents.get(app.selected) {
                                        let host = agent.host.clone();
                                        let user = agent.ssh_user.clone();
                                        let self_ip = app.self_ip.clone();
                                        let is_mac = agent.os.to_lowercase().contains("mac");
                                        app.toast("📋 Fetching config...");
                                        let (tx, rx) = mpsc::unbounded_channel();
                                        app.config_load_rx = Some(rx);
                                        tokio::spawn(async move {
                                            let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                            let cmd = format!("{}cat ~/.openclaw/openclaw.json 2>/dev/null || echo '(no config found)'", pfx);
                                            let output = if host == "localhost" || host == self_ip {
                                                tokio::process::Command::new("bash").args(["-c", &cmd]).output().await.ok()
                                            } else {
                                                tokio::time::timeout(
                                                    std::time::Duration::from_secs(5),
                                                    tokio::process::Command::new("ssh")
                                                        .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                            &format!("{}@{}", user, host), &cmd])
                                                        .output()
                                                ).await.ok().and_then(|r| r.ok())
                                            };
                                            let _ = tx.send(output.map(|o| String::from_utf8_lossy(&o.stdout).to_string()));
                                        });
                                    }
                                }
                                KeyCode::Char('c') => app.cycle_theme(),
                                KeyCode::Char('l') => {
                                    // Fetch gateway logs for this agent
                                    if let Some(agent) = app.agents.get(app.selected) {
                                        let host = agent.host.clone();
                                        let user = agent.ssh_user.clone();
                                        let name = agent.db_name.clone();
                                        let self_ip = app.self_ip.clone();
                                        if let Some(pool) = &app.db_pool {
                                            let pool = pool.clone();
                                            let sender = app.user();
                                            tokio::spawn(async move {
                                                let cmd = "journalctl -u openclaw-gateway --no-pager -n 15 --output=short-iso 2>/dev/null || openclaw gateway status 2>/dev/null || echo 'no logs available'";
                                                let output = if host == "localhost" || host == self_ip {
                                                    tokio::process::Command::new("bash").args(["-c", cmd]).output().await.ok()
                                                } else {
                                                    let is_mac = host.contains("mac") || host.contains("darwin");
                                                    let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                                    tokio::time::timeout(
                                                        std::time::Duration::from_secs(5),
                                                        tokio::process::Command::new("ssh")
                                                            .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                                &format!("{}@{}", user, host), &format!("{}{}", pfx, cmd)])
                                                            .output()
                                                    ).await.ok().and_then(|r| r.ok())
                                                };
                                                let response = output.map(|o| {
                                                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                                    if s.is_empty() { "(no output)".into() } else { s.chars().take(1000).collect::<String>() }
                                                }).unwrap_or_else(|| "Timeout".into());
                                                let _ = crate::db::send_direct(&pool, &sender, &name, "📋 gateway logs").await;
                                                if let Ok(mut conn) = pool.get_conn().await {
                                                    use mysql_async::prelude::*;
                                                    let _ = conn.exec_drop(
                                                        "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW() WHERE sender=? AND target=? AND status='pending' ORDER BY id DESC LIMIT 1",
                                                        (&response, &sender, &name),
                                                    ).await;
                                                }
                                            });
                                        }
                                        let agent_name = app.agents.get(app.selected).map(|a| a.name.clone()).unwrap_or_default();
                                        app.status_message = format!("📋 Fetching gateway logs from {}...", agent_name);
                                    }
                                }
                                _ => {}
                            },
                        },
                        Screen::Alerts => match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => { app.screen = Screen::Dashboard; app.focus = Focus::Fleet; }
                            KeyCode::Char('b') => app.cycle_bg(),
                            KeyCode::Char('c') => app.cycle_theme(),
                            _ => {}
                        },
                        Screen::VpnStatus => match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => { app.screen = Screen::Dashboard; app.focus = Focus::Fleet; }
                            KeyCode::Char('b') => app.cycle_bg(),
                            KeyCode::Char('c') => app.cycle_theme(),
                            _ => {}
                        },
                        Screen::TaskBoard => {
                            if app.task_input_active {
                                match key.code {
                                    KeyCode::Esc => app.task_input_active = false,
                                    KeyCode::Enter => {
                                        if !app.task_input.trim().is_empty() {
                                            let desc = app.task_input.clone();
                                            app.task_input.clear();
                                            app.task_input_active = false;
                                            if let Some(pool) = &app.db_pool {
                                                let agent = app.task_filter_agent.as_deref(); let _ = db::create_task(pool, &desc, 5, &app.user(), agent).await;
if let Ok(tasks) = db::load_tasks(pool, 50).await { app.tasks = tasks; }
                                            }
                                            app.toast("✓ Task created");
                                        }
                                    }
                                    KeyCode::Backspace => { app.task_input.pop(); }
                                    KeyCode::Char(ch) => app.task_input.push(ch),
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc => {
                                        if app.task_filter_agent.is_some() {
                                            app.screen = Screen::AgentDetail;
                                            app.focus = Focus::Fleet;
                                        } else {
                                            app.screen = Screen::Dashboard;
                                            app.focus = Focus::Fleet;
                                        }
                                    }
                                    KeyCode::Char('q') => app.should_quit = true,
                                    KeyCode::Char('1') if app.task_filter_agent.is_some() => { app.screen = Screen::AgentDetail; app.focus = Focus::Fleet; }
                                    KeyCode::Char('2') if app.task_filter_agent.is_some() => { app.screen = Screen::AgentDetail; app.focus = Focus::AgentChat; }
                                    KeyCode::Char('3') if app.task_filter_agent.is_some() => { app.screen = Screen::AgentDetail; app.focus = Focus::Workspace; app.start_workspace_load(); }
                                    KeyCode::Char('4') => {} // already on tasks
                                    KeyCode::Char('5') if app.task_filter_agent.is_some() => { app.screen = Screen::AgentDetail; app.focus = Focus::Services; app.start_services_load(); }
                                    KeyCode::Up | KeyCode::Char('k') => { if app.task_selected > 0 { app.task_selected -= 1; } }
                                    KeyCode::Down | KeyCode::Char('j') => { if app.task_selected < app.tasks.len().saturating_sub(1) { app.task_selected += 1; } }
                                    KeyCode::Char('n') => app.task_input_active = true,
                                    KeyCode::Char('d') => {
                                        if let Some(task) = app.tasks.get(app.task_selected) {
                                            let tid = task.id;
                                            if let Some(pool) = &app.db_pool {
                                                let _ = db::update_task_status(pool, tid, "completed").await;
                                                if let Ok(tasks) = db::load_tasks(pool, 50).await { app.tasks = tasks; }
                                            }
                                        }
                                    }
                                    KeyCode::Char('c') if app.task_filter_agent.is_some() => {
                                        app.task_filter_agent = None;
                                        app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                        app.toast("Filter cleared — showing all tasks");
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Screen::Dashboard => match app.focus {
                            Focus::Fleet if app.filter_active => match key.code {
                                KeyCode::Esc => { app.filter_active = false; app.filter_text.clear(); }
                                KeyCode::Enter => { app.filter_active = false; }
                                KeyCode::Backspace => { app.filter_text.pop(); }
                                KeyCode::Char(ch) => { app.filter_text.push(ch); }
                                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                                KeyCode::Down | KeyCode::Char('j') => app.next(),
                                _ => {}
                            },
                            Focus::Fleet => match key.code {
                                KeyCode::Char('q') => app.should_quit = true,
                                KeyCode::Tab => app.focus = Focus::Chat,
                                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                                KeyCode::Down | KeyCode::Char('j') => app.next(),
                                KeyCode::Enter => {
                                    app.screen = Screen::AgentDetail;
                                    app.focus = Focus::Fleet;
                                    app.agent_chat_input.clear();
                                    app.agent_chat_history.clear();
                                    app.agent_chat_scroll = 0;
                                    // Trigger immediate agent chat load
                                    app.last_chat_poll = Instant::now() - Duration::from_secs(10);
                                }
                                KeyCode::Char(' ') => {
                                    if app.multi_selected.contains(&app.selected) {
                                        app.multi_selected.remove(&app.selected);
                                    } else {
                                        app.multi_selected.insert(app.selected);
                                    }
                                    app.next();
                                }
                                KeyCode::Char('f') => {
                                    app.filter_active = true;
                                    app.filter_text.clear();
                                }
                                KeyCode::Char('?') => app.screen = Screen::Help,
                                KeyCode::Char('r') => app.start_refresh(),
                                KeyCode::Char('b') => app.cycle_bg(),
                                KeyCode::Char('c') => app.cycle_theme(),
                                KeyCode::Char('s') => { app.cycle_sort(); app.toast(&format!("Sort: {}{}", app.sort_mode.label(), app.sort_mode.arrow())); }
                                KeyCode::Char('a') => { app.wizard.open(); }
                                KeyCode::Char('A') => {
                                    // Select all
                                    for i in 0..app.agents.len() { app.multi_selected.insert(i); }
                                    app.toast(&format!("✓ Selected all {} agents", app.agents.len()));
                                }
                                KeyCode::Char('N') => {
                                    app.multi_selected.clear();
                                    app.toast("Selection cleared");
                                }
                                KeyCode::Char('h') => {
                                    // Fleet health summary
                                    let total = app.agents.len();
                                    let online = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
                                    let offline: Vec<String> = app.agents.iter()
                                        .filter(|a| a.status == AgentStatus::Offline)
                                        .map(|a| a.name.clone()).collect();
                                    let unknown: Vec<String> = app.agents.iter()
                                        .filter(|a| a.status == AgentStatus::Unknown)
                                        .map(|a| a.name.clone()).collect();
                                    let outdated: Vec<String> = app.agents.iter()
                                        .filter(|a| !a.oc_version.is_empty() && a.oc_version != "2026.2.21-2" && a.oc_version != "?")
                                        .map(|a| format!("{}({})", a.name, a.oc_version)).collect();

                                    let mut msg = format!("🏥 {}/{} online", online, total);
                                    if !offline.is_empty() { msg += &format!(" │ ❌ offline: {}", offline.join(", ")); }
                                    if !unknown.is_empty() { msg += &format!(" │ ❓ unknown: {}", unknown.join(", ")); }
                                    if !outdated.is_empty() { msg += &format!(" │ ⚠️  old OC: {}", outdated.join(", ")); }
                                    if offline.is_empty() && unknown.is_empty() && outdated.is_empty() { msg += " │ ✅ All healthy"; }
                                    app.status_message = msg;
                                }
                                KeyCode::Char('/') => {
                                    app.focus = Focus::Command;
                                    app.command_input.clear();
                                }
                                KeyCode::Char('o') => {
                                    // OpenClaw fleet operations menu
                                    app.status_message = "⏳ Running OC audit...".into();
                                    let mut outdated = vec![];
                                    let latest = "2026.2.21-2";
                                    for agent in &app.agents {
                                        if !agent.oc_version.is_empty() && agent.oc_version != latest && agent.oc_version != "?" {
                                            outdated.push(format!("{} ({})", agent.name, agent.oc_version));
                                        }
                                    }
                                    if outdated.is_empty() {
                                        app.status_message = format!("✅ All agents on {}", latest);
                                    } else {
                                        app.status_message = format!("⚠️  {} outdated: {}", outdated.len(), outdated.join(", "));
                                    }
                                }
                                KeyCode::Char('u') => {
                                    // Bulk update OC on all agents
                                    app.status_message = "🔄 Updating OpenClaw fleet-wide...".into();
                                    let targets: Vec<&Agent> = if app.multi_selected.is_empty() {
                                        app.agents.iter().collect()
                                    } else {
                                        app.multi_selected.iter().filter_map(|&i| app.agents.get(i)).collect()
                                    };
                                    for agent in targets {
                                        if agent.host == "localhost" || agent.host == app.self_ip { continue; }
                                        let host = agent.host.clone();
                                        let user = agent.ssh_user.clone();
                                        let is_mac = agent.os.to_lowercase().contains("mac");
                                        tokio::spawn(async move {
                                            let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                            let cmd = format!("{}sudo npm install -g openclaw@latest 2>&1 | tail -1", pfx);
                                            let _ = tokio::time::timeout(
                                                std::time::Duration::from_secs(60),
                                                tokio::process::Command::new("ssh")
                                                    .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                        &format!("{}@{}", user, host), &cmd])
                                                    .output()
                                            ).await;
                                        });
                                    }
                                    app.status_message = "🔄 OC update dispatched to all agents (background)".into();
                                }
                                KeyCode::Char('G') => {
                                    // Gateway status on selected agent
                                    if let Some(agent) = app.agents.get(app.selected) {
                                        let host = agent.host.clone();
                                        let user = agent.ssh_user.clone();
                                        let name = agent.name.clone();
                                        let self_ip = app.self_ip.clone();
                                        let is_mac = agent.os.to_lowercase().contains("mac");
                                        app.status_message = format!("🔍 Checking gateway on {}...", name);
                                        if let Some(pool) = &app.db_pool {
                                            let pool = pool.clone();
                                            let sender = app.user();
                                            let db_name = agent.db_name.clone();
                                            tokio::spawn(async move {
                                                let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                                let cmd = format!("{}echo '=== Gateway Status ===' && openclaw gateway status 2>&1 && echo '=== OC Version ===' && openclaw --version 2>&1 && echo '=== Last 5 Log Lines ===' && journalctl -u openclaw-gateway --no-pager -n 5 --output=short-iso 2>/dev/null || echo 'no systemd logs'", pfx);
                                                let output = if host == "localhost" || host == self_ip {
                                                    tokio::process::Command::new("bash").args(["-c", &cmd]).output().await.ok()
                                                } else {
                                                    tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        tokio::process::Command::new("ssh")
                                                            .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                                &format!("{}@{}", user, host), &cmd])
                                                            .output()
                                                    ).await.ok().and_then(|r| r.ok())
                                                };
                                                let response = output.map(|o| {
                                                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                                    if s.is_empty() { "(no output)".into() } else { s.chars().take(1500).collect::<String>() }
                                                }).unwrap_or_else(|| "Timeout".into());
                                                let _ = crate::db::send_direct(&pool, &sender, &db_name, "🔍 gateway investigate").await;
                                                if let Ok(mut conn) = pool.get_conn().await {
                                                    use mysql_async::prelude::*;
                                                    let _ = conn.exec_drop(
                                                        "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW() WHERE sender=? AND target=? AND status='pending' ORDER BY id DESC LIMIT 1",
                                                        (&response, &sender, &db_name),
                                                    ).await;
                                                }
                                            });
                                        }
                                    }
                                }
                                KeyCode::Char('g') => {
                                    // Restart gateway on focused agent — requires two presses within 5s
                                    if let Some(agent) = app.agents.get(app.selected) {
                                        let name = agent.name.clone();
                                        let confirmed = app.gateway_confirm_at
                                            .map(|t| t.elapsed().as_secs() < 5)
                                            .unwrap_or(false);
                                        if confirmed {
                                            app.gateway_confirm_at = None;
                                            let host = agent.host.clone();
                                            let user = agent.ssh_user.clone();
                                            let is_mac = agent.os.to_lowercase().contains("mac");
                                            app.status_message = format!("🔄 Restarting gateway on {}...", name);
                                            tokio::spawn(async move {
                                                let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                                let cmd = format!("{}openclaw gateway restart 2>&1 | tail -1", pfx);
                                                let _ = tokio::process::Command::new("ssh")
                                                    .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                        &format!("{}@{}", user, host), &cmd])
                                                    .output().await;
                                            });
                                        } else {
                                            app.gateway_confirm_at = Some(Instant::now());
                                            app.toast(&format!("⚠ Press g again to restart gateway on {}", name));
                                        }
                                    }
                                }
                                KeyCode::Char('w') => {
                                    app.screen = Screen::Alerts;
                                }
                                KeyCode::Char('v') => {
                                    app.screen = Screen::VpnStatus;
                                }
                                KeyCode::Char('x') => app.screen = Screen::SpawnManager,
                                KeyCode::Char('t') => {
                                    app.task_filter_agent = None;
                                    app.screen = Screen::TaskBoard;
                                    app.last_task_poll = Instant::now() - Duration::from_secs(10);
                                }
                                _ => {}
                            },
                            Focus::Command => match key.code {
                                KeyCode::Esc => { app.focus = Focus::Fleet; app.command_input.clear(); }
                                KeyCode::Enter => {
                                    if !app.command_input.trim().is_empty() {
                                        let cmd = app.command_input.clone();
                                        app.command_input.clear();
                                        app.focus = Focus::Fleet;
                                        app.status_message = format!("⚡ Running '{}' on all agents...", &cmd);

                                        // Fan out to selected agents (or all online if none selected)
                                        let agents: Vec<(String, String, String, bool)> = if app.multi_selected.is_empty() {
                                            app.agents.iter()
                                                .filter(|a| a.status == AgentStatus::Online)
                                                .map(|a| (a.db_name.clone(), a.host.clone(), a.ssh_user.clone(), a.os.to_lowercase().contains("mac")))
                                                .collect()
                                        } else {
                                            app.multi_selected.iter()
                                                .filter_map(|&i| app.agents.get(i))
                                                .filter(|a| a.status == AgentStatus::Online)
                                                .map(|a| (a.db_name.clone(), a.host.clone(), a.ssh_user.clone(), a.os.to_lowercase().contains("mac")))
                                                .collect()
                                        };

                                        if let Some(pool) = &app.db_pool {
                                            let user = app.user();
                                            for (name, host, ssh_user, is_mac) in agents {
                                                let pool = pool.clone();
                                                let cmd = cmd.clone();
                                                let user = user.clone();
                                                let self_ip = app.self_ip.clone();
                                                tokio::spawn(async move {
                                                    let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                                    let full_cmd = format!("{}{}", pfx, cmd);

                                                    let output = if host == "localhost" || host == self_ip {
                                                        tokio::process::Command::new("bash").args(["-c", &cmd])
                                                            .output().await.ok()
                                                    } else {
                                                        tokio::time::timeout(
                                                            std::time::Duration::from_secs(10),
                                                            tokio::process::Command::new("ssh")
                                                                .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                                    &format!("{}@{}", ssh_user, host), &full_cmd])
                                                                .output()
                                                        ).await.ok().and_then(|r| r.ok())
                                                    };

                                                    let response = match output {
                                                        Some(o) => {
                                                            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                                            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                                                            if out.is_empty() && err.is_empty() { "(no output)".into() }
                                                            else if out.is_empty() { err.chars().take(500).collect::<String>() }
                                                            else { out.chars().take(500).collect::<String>() }
                                                        }
                                                        None => "Timeout/error".into(),
                                                    };

                                                    // Write result to mc_chat
                                                    let _ = crate::db::send_direct(&pool, &user, &name, &format!("/{}", cmd)).await;
                                                    // Update the last message with the response
                                                    if let Ok(mut conn) = pool.get_conn().await {
                                                        use mysql_async::prelude::*;
                                                        let _ = conn.exec_drop(
                                                            "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW() WHERE sender=? AND target=? AND status='pending' ORDER BY id DESC LIMIT 1",
                                                            (&response, &user, &name),
                                                        ).await;
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }
                                KeyCode::Backspace => { app.command_input.pop(); }
                                KeyCode::Char(ch) => app.command_input.push(ch),
                                _ => {}
                            },
                            Focus::Chat => if app.ac_visible {
                                match key.code {
                                    KeyCode::Up => { if app.ac_selected > 0 { app.ac_selected -= 1; } else { app.ac_selected = app.ac_matches.len().saturating_sub(1); } }
                                    KeyCode::Down => { app.ac_selected = (app.ac_selected + 1) % app.ac_matches.len().max(1); }
                                    KeyCode::Tab | KeyCode::Enter => app.accept_autocomplete(),
                                    KeyCode::Esc => { app.ac_visible = false; }
                                    KeyCode::Backspace => { app.chat_input.pop(); app.update_autocomplete(); }
                                    KeyCode::Char(c) => { app.chat_input.push(c); app.update_autocomplete(); }
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Tab | KeyCode::Esc => app.focus = Focus::Fleet,
                                    KeyCode::Enter => app.send_message().await,
                                    KeyCode::Backspace => { app.chat_input.pop(); app.update_autocomplete(); }
                                    KeyCode::Char(c) => { app.chat_input.push(c); app.update_autocomplete(); }
                                    KeyCode::PageUp => app.chat_scroll = app.chat_scroll.saturating_add(5),
                                    KeyCode::PageDown => app.chat_scroll = app.chat_scroll.saturating_sub(5),
                                    _ => {}
                                }
                            },
                            _ => {}
                        },
                    }
                }
                    }
                    } // close else for diag_active
            }
        }

        // Auto-dismiss splash
        if app.show_splash && app.splash_start.elapsed() > Duration::from_secs(3) {
            app.show_splash = false;
        }

        // Auto-refresh every 30s (non-blocking)
        if app.last_refresh.elapsed() > Duration::from_secs(30) && !app.refreshing {
            app.start_refresh();
        }

        // Poll tasks every 5s when on task board
        if app.screen == Screen::TaskBoard && app.last_task_poll.elapsed() > Duration::from_secs(5) {
            if let Some(pool) = &app.db_pool {
                if let Ok(mut tasks) = db::load_tasks(pool, 50).await {
                    if let Some(ref agent) = app.task_filter_agent {
                        tasks.retain(|t| t.assigned_agent.as_ref().map(|a| a == agent).unwrap_or(false));
                    }
                    app.tasks = tasks;
                }
            }
            app.last_task_poll = Instant::now();
        }

        // Receive diagnostic steps (non-blocking)
        if app.diag_active {
            if let Some(ref mut rx) = app.diag_rx {
                while let Ok(step) = rx.try_recv() {
                    let is_done = step.label == "DONE";
                    app.diag_steps.push(step);
                    if is_done {
                        // Keep overlay open for user to see results
                    }
                }
            }
        }

        // Receive config load results (non-blocking)
        if let Some(ref mut rx) = app.config_load_rx {
            if let Ok(result) = rx.try_recv() {
                app.config_text = result;
                app.config_scroll = 0;
                app.toast("📋 Config loaded — PageUp/Down to scroll, Esc to close");
            }
        }

        // Receive services load results (non-blocking)
        if let Some(ref mut rx) = app.svc_load_rx {
            if let Ok(config) = rx.try_recv() {
                app.svc_config = config;
                app.parse_services();
                app.svc_loading = false;
                let count = app.svc_list.len();
                app.toast(&format!("✓ Loaded {} services", count));
            }
        }

        // Receive workspace load results (non-blocking)
        if let Some(ref mut rx) = app.ws_load_rx {
            if let Ok((files, crons)) = rx.try_recv() {
                app.ws_files = files;
                app.ws_crons = crons;
                app.ws_loading = false;
                let found = app.ws_files.iter().filter(|f| f.exists).count();
                app.toast(&format!("✓ Loaded workspace — {}/{} files found", found, app.ws_files.len()));
            }
        }
        if let Some(ref mut rx) = app.ws_file_rx {
            if let Ok(content) = rx.try_recv() {
                app.ws_content = Some(content);
                app.ws_content_scroll = 0;
            }
        }

        // Receive background chat poll results (non-blocking)
        let mut poll_results: Vec<ChatPollResult> = Vec::new();
        if let Some(ref mut rx) = app.chat_poll_rx {
            while let Ok(result) = rx.try_recv() {
                poll_results.push(result);
            }
        }
        for result in poll_results {
            app.chat_history = result.global;
            if let Some(agent_msgs) = result.agent {
                app.agent_chat_history = agent_msgs;
            }
            for id in &result.new_routed_ids {
                app.routed_msg_ids.insert(*id);
            }
            for (sender, response) in result.to_route {
                app.route_agent_mentions(&sender, &response).await;
            }
            app.chat_polling = false;
        }

        // Spawn background chat poll (never blocks main loop)
        let has_pending = app.chat_history.iter().chain(app.agent_chat_history.iter())
            .any(|m| matches!(m.status.as_str(), "pending" | "connecting" | "thinking" | "streaming" | "processing" | "routing"));
        let poll_interval = if has_pending { Duration::from_millis(400) } else { Duration::from_secs(3) };
        if !app.chat_polling && app.last_chat_poll.elapsed() > poll_interval {
            if let Some(pool) = app.db_pool.clone() {
                app.chat_polling = true;
                app.last_chat_poll = Instant::now();
                let (tx, rx) = mpsc::unbounded_channel();
                app.chat_poll_rx = Some(rx);
                let user = app.user();
                let routed = app.routed_msg_ids.clone();
                let on_detail = app.screen == Screen::AgentDetail && app.selected < app.agents.len();
                let agent_name = if on_detail { Some(app.agents[app.selected].db_name.clone()) } else { None };

                tokio::spawn(async move {
                    let mut to_route: Vec<(String, String)> = Vec::new();
                    let mut new_routed = Vec::new();

                    // Global chat
                    let global = if let Ok(msgs) = db::load_global_chat(&pool, 100).await {
                        for m in &msgs {
                            if m.status == "responded" && m.sender != user && !routed.contains(&m.id) {
                                if let Some(ref resp) = m.response {
                                    if resp.contains('@') {
                                        new_routed.push(m.id);
                                        to_route.push((m.sender.clone(), resp.clone()));
                                    }
                                }
                            }
                        }
                        msgs.iter().map(|m| ChatLine {
                            sender: m.sender.clone(), target: m.target.clone(),
                            message: m.message.clone(), response: m.response.clone(),
                            time: m.created_at.clone(), status: m.status.clone(),
                            kind: m.kind.clone(),
                        }).collect()
                    } else { vec![] };

                    // Agent chat
                    let agent = if let Some(ref name) = agent_name {
                        if let Ok(msgs) = db::load_agent_chat(&pool, name, 100).await {
                            for m in &msgs {
                                if m.status == "responded" && m.sender != user && !routed.contains(&m.id) {
                                    if let Some(ref resp) = m.response {
                                        if resp.contains('@') {
                                            new_routed.push(m.id);
                                            to_route.push((m.sender.clone(), resp.clone()));
                                        }
                                    }
                                }
                            }
                            Some(msgs.iter().map(|m| ChatLine {
                                sender: m.sender.clone(), target: m.target.clone(),
                                message: m.message.clone(), response: m.response.clone(),
                                time: m.created_at.clone(), status: m.status.clone(),
                                kind: m.kind.clone(),
                            }).collect())
                        } else { None }
                    } else { None };

                    let _ = tx.send(ChatPollResult { global, agent, to_route, new_routed_ids: new_routed });
                });

                // Record routed IDs (we'll also get them from the result, but pre-mark to avoid dupes)
            }
        }

        if app.should_quit { break; }
    }

    if let Some(pool) = app.db_pool.take() { pool.disconnect().await?; }
    disable_raw_mode()?;
    stdout().execute(crossterm::event::DisableMouseCapture)?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::INPUT_POLL_MS;

    #[test]
    fn input_poll_interval_is_low_for_responsive_ui() {
        assert!(INPUT_POLL_MS <= 10);
    }
}
