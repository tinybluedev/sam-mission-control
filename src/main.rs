mod cli;
mod config;
mod wizard;
mod db;
mod theme;

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
enum Focus { Fleet, Chat, AgentChat, Command }

#[derive(PartialEq)]
enum Screen { Dashboard, AgentDetail, TaskBoard, VpnStatus, Help }

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
}

struct ProbeResult {
    index: usize,
    status: AgentStatus,
    os: String,
    kernel: String,
    oc_version: String,
    latency_ms: Option<u32>,
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
    // Multi-select
    multi_selected: std::collections::HashSet<usize>,
    // Theme
    theme_name: ThemeName,
    bg_density: BgDensity,
    theme: Theme,
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
                        emoji: cfg.map(|c| c.emoji().to_string()).unwrap_or_else(|| "❓".into()),
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
            status_message: String::new(),
            db_pool: Some(pool),
            chat_input: String::new(), chat_history, chat_scroll: 0,
            agent_chat_input: String::new(), agent_chat_history: vec![], agent_chat_scroll: 0,
            refresh_rx: None, refreshing: false, self_ip,
            command_input: String::new(),
            wizard: wizard::AgentWizard::new(),
            tasks: vec![], task_selected: 0, task_input: String::new(), task_input_active: false,
            last_task_poll: Instant::now(),
            multi_selected: HashSet::new(),
            spinner_frame: 0, sort_mode: SortMode::Name,
            fleet_area: Rect::default(), chat_area: Rect::default(),
            detail_info_area: Rect::default(), detail_chat_area: Rect::default(),
            fleet_row_start_y: 0,
            theme_name: tn, bg_density: bd, theme: Theme::resolve(tn, bd),
        }
    }

    fn next(&mut self) { if self.selected < self.agents.len() - 1 { self.selected += 1; } }
    fn previous(&mut self) { if self.selected > 0 { self.selected -= 1; } }

    fn user(&self) -> String { std::env::var("SAM_USER").unwrap_or_else(|_| "nick".into()) }

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

    fn agent_chat_lines(&self) -> &Vec<ChatLine> {
        &self.agent_chat_history
    }

    async fn send_message(&mut self) {
        if self.chat_input.trim().is_empty() { return; }
        let message = self.chat_input.clone();
        self.chat_input.clear();

        // Dashboard chat = broadcast to all agents
        let agent_names: Vec<String> = self.agents.iter().map(|a| a.db_name.clone()).collect();
        self.chat_history.push(ChatLine {
            sender: self.user(), target: None, message: message.clone(),
            response: None, time: now_str(), status: "pending".into(),
            kind: "global".into(),
        });

        if let Some(pool) = &self.db_pool {
            let _ = db::send_broadcast(pool, &self.user(), &message, &agent_names).await;
        }
        self.chat_scroll = 0;
    }

    async fn send_agent_message(&mut self) {
        if self.agent_chat_input.trim().is_empty() { return; }
        let message = self.agent_chat_input.clone();
        self.agent_chat_input.clear();
        let target = self.agents[self.selected].db_name.clone();

        self.agent_chat_history.push(ChatLine {
            sender: self.user(), target: Some(target.clone()), message: message.clone(),
            response: None, time: now_str(), status: "pending".into(),
            kind: "direct".into(),
        });

        if let Some(pool) = &self.db_pool {
            let _ = db::send_chat(pool, &self.user(), Some(&target), &message).await;
        }
        self.agent_chat_scroll = 0;
    }

    async fn poll_chat(&mut self) {
        if let Some(pool) = &self.db_pool {
            // Global chat for dashboard
            if let Ok(msgs) = db::load_global_chat(pool, 100).await {
                self.chat_history = msgs.iter().map(|m| ChatLine {
                    sender: m.sender.clone(), target: m.target.clone(),
                    message: m.message.clone(), response: m.response.clone(),
                    time: m.created_at.clone(), status: m.status.clone(),
                    kind: m.kind.clone(),
                }).collect();
            }
            // Agent-specific chat (if on detail screen)
            if self.screen == Screen::AgentDetail && self.selected < self.agents.len() {
                let agent = &self.agents[self.selected].db_name;
                if let Ok(msgs) = db::load_agent_chat(pool, agent, 100).await {
                    self.agent_chat_history = msgs.iter().map(|m| ChatLine {
                        sender: m.sender.clone(), target: m.target.clone(),
                        message: m.message.clone(), response: m.response.clone(),
                        time: m.created_at.clone(), status: m.status.clone(),
                        kind: m.kind.clone(),
                    }).collect();
                }
            }
        }
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
                let (status, os, kern, oc, lat) = probe_agent(&host, &user, &sip).await;
                let _ = tx.send(ProbeResult { index: i, status, os, kernel: kern, oc_version: oc, latency_ms: lat });
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
                    self.agents[r.index].last_seen = now_str();
                    updates.push((r.index, r.status, r.os, r.kernel, r.oc_version, r.latency_ms));
                }
            }
        }
        if self.refreshing && self.last_refresh.elapsed() > Duration::from_secs(8) {
            self.refreshing = false;
        }
        updates
    }

    fn update_status_bar(&mut self) {
        let on = self.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
        let total = self.agents.len();
        let spinner_chars = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
        let refresh = if self.refreshing {
            self.spinner_frame = (self.spinner_frame + 1) % spinner_chars.len();
            format!(" {} ", spinner_chars[self.spinner_frame])
        } else { String::new() };
        let chat_count = self.chat_history.len();
        let sel_info = if !self.multi_selected.is_empty() {
            format!(" │ 🔲 {}", self.multi_selected.len())
        } else { String::new() };
        self.status_message = format!(
            "v1.0 │ {}/{} online{}{} │ sort:{} │ chat({}) │ {}/{} │ /=cmd ?=help",
            on, total, refresh, sel_info, self.sort_mode.label(), chat_count,
            self.theme_name.label(), self.bg_density.label()
        );
    }
}

// ---- SSH Probe ----

async fn probe_agent(host: &str, user: &str, self_ip: &str) -> (AgentStatus, String, String, String, Option<u32>) {
    let start = Instant::now();
    if host == "localhost" || host == self_ip {
        let os = Command::new("bash").args(["-c", ". /etc/os-release 2>/dev/null && echo \"$NAME $VERSION_ID\" || echo unknown"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let kern = Command::new("uname").arg("-r").output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let oc = Command::new("bash").args(["-c", "openclaw --version 2>/dev/null || echo ?"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let ms = start.elapsed().as_millis() as u32;
        return (AgentStatus::Online, os, kern, oc, Some(ms));
    }
    let tgt = format!("{}@{}", user, host);
    let script = r#"OS=$(. /etc/os-release 2>/dev/null && echo "$NAME $VERSION_ID" || (sw_vers -productName 2>/dev/null; sw_vers -productVersion 2>/dev/null) || echo ?); KERN=$(uname -r); OC=$(openclaw --version 2>/dev/null || echo ?); echo "OS:$OS"; echo "KERN:$KERN"; echo "OC:$OC""#;
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        Command::new("ssh").args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",&tgt,"bash","-c",script]).output()
    ).await;
    let result = match result {
        Ok(r) => r,
        Err(_) => return (AgentStatus::Offline, String::new(), String::new(), String::new(), None),
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
            let ms = start.elapsed().as_millis() as u32;
            (AgentStatus::Online, os, kern, oc, Some(ms))
        }
        _ => (AgentStatus::Offline, String::new(), String::new(), String::new(), None),
    }
}

fn now_str() -> String {
    use std::process::Command as C;
    C::new("date").arg("+%H:%M:%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("now".into())
}

// ---- Chat Line Rendering ----

fn build_chat_lines(messages: &[ChatLine], user: &str, t: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if messages.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  No messages yet".to_string(), Style::default().fg(t.text_dim))));
        return lines;
    }
    for msg in messages {
        let ts = msg.target.as_ref().map(|t| format!("→@{}", t)).unwrap_or_else(|| "→all".into());
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", msg.time), Style::default().fg(t.text_dim)),
            Span::styled(msg.sender.clone(), Style::default().fg(
                if msg.sender == user { t.sender_self } else { t.sender_other }
            ).bold()),
            Span::styled(format!(" {}", ts), Style::default().fg(t.text_dim)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("     "),
            Span::styled(msg.message.clone(), Style::default().fg(t.text)),
        ]));
        if let Some(resp) = &msg.response {
            let max_w = 50;
            let words: Vec<&str> = resp.split_whitespace().collect();
            let (mut cur, mut first) = (String::new(), true);
            for w in &words {
                if cur.len() + w.len() + 1 > max_w && !cur.is_empty() {
                    let prefix = if first { "↳ ".to_string() } else { "  ".to_string() };
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(prefix, Style::default().fg(t.sender_other)),
                        Span::styled(cur.clone(), Style::default().fg(t.response)),
                    ]));
                    cur.clear(); first = false;
                }
                if !cur.is_empty() { cur.push(' '); }
                cur.push_str(w);
            }
            if !cur.is_empty() {
                let prefix = if first { "↳ ".to_string() } else { "  ".to_string() };
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(prefix, Style::default().fg(t.sender_other)),
                    Span::styled(cur, Style::default().fg(t.response)),
                ]));
            }
        } else {
            let status_text = match msg.status.as_str() {
                "pending" => "⏳ pending...",
                "processing" => "🔄 processing...",
                "thinking" => "💭 thinking...",
                "received" => "📨 received",
                _ => "",
            };
            if !status_text.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(status_text.to_string(), Style::default().fg(t.pending)),
                ]));
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

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🛰️  S.A.M MISSION CONTROL", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("{}", online), Style::default().fg(t.status_online).bold()),
        Span::styled(format!("/{} agents", total), Style::default().fg(t.text_dim)),
        Span::raw("    "),
        Span::styled(if live { "● live" } else { "○ stale" }, Style::default().fg(if live { t.status_online } else { t.status_offline })),
        Span::raw("    "),
        Span::styled(if app.refreshing { "⟳ refreshing" } else { "" }, Style::default().fg(t.accent)),
        Span::raw("    "),
        Span::styled(match app.focus {
            Focus::Fleet => "▌Fleet▐", Focus::Chat => "▌Chat▐", _ => "▌Fleet▐",
        }, Style::default().fg(t.accent).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
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

fn render_fleet_table(frame: &mut Frame, app: &mut App, area: Rect, active: bool) {
    let t = &app.theme;
    let fb = if active { t.border_active } else { t.border };

    let show_latency = area.width > 70;
    let hcells_vec: Vec<&str> = if show_latency {
        vec!["  ", "Agent", "Location", "Status", "Ping", "Version"]
    } else {
        vec!["  ", "Agent", "Location", "Status", "Version"]
    };
    let hcells = hcells_vec.iter().map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.agents.iter().enumerate().map(|(i, a)| {
        let sel = i == app.selected && active;
        let bg = if sel { t.selected_bg } else { app.bg_density.bg() };
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
            Cell::from(a.location.clone()).style(Style::default().fg(loc_color)),
            Cell::from(a.status.to_string()).style(Style::default().fg(st_color)),
        ];
        if show_latency {
            cells.push(Cell::from(lat_str).style(Style::default().fg(lat_color)));
        }
        cells.push(Cell::from(a.oc_version.clone()).style(Style::default().fg(t.version)));
        Row::new(cells).style(Style::default().bg(bg)).height(1)
    }).collect();

    app.fleet_row_start_y = area.y + 1; // +1 for border, +1 for header handled in click calc

    let show_version = area.width > 55;
    let widths = if show_latency && show_version {
        vec![Constraint::Length(4), Constraint::Length(14), Constraint::Length(9), Constraint::Length(12), Constraint::Length(7), Constraint::Min(12)]
    } else if show_latency {
        vec![Constraint::Length(4), Constraint::Length(14), Constraint::Length(9), Constraint::Min(12), Constraint::Length(7), Constraint::Length(0)]
    } else if show_version {
        vec![Constraint::Length(4), Constraint::Length(14), Constraint::Length(9), Constraint::Length(12), Constraint::Min(12)]
    } else {
        vec![Constraint::Length(4), Constraint::Length(14), Constraint::Length(9), Constraint::Min(12), Constraint::Length(0)]
    };
    let table = Table::new(rows, widths).header(hrow)
    .block(Block::default().title(Span::styled(" Fleet ", Style::default().fg(fb).bold()))
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(fb))
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

    let (messages, scroll, input_text) = if agent_mode {
        let msgs = app.agent_chat_lines();
        let lines = build_chat_lines(msgs, &app.user(), t);
        (lines, app.agent_chat_scroll, &app.agent_chat_input)
    } else {
        let lines = build_chat_lines(&app.chat_history, &app.user(), t);
        (lines, app.chat_scroll, &app.chat_input)
    };

    let vh = cl[0].height.saturating_sub(2) as usize;
    let tl = messages.len();
    let scroll_pos = if tl > vh && scroll == 0 { (tl - vh) as u16 } else { scroll };

    let title = if agent_mode {
        format!(" {} {} Chat ", app.agents[app.selected].emoji, app.agents[app.selected].name)
    } else {
        let count = app.chat_history.len();
        if count > 0 { format!(" Chat ({}) ", count) } else { " Chat ".to_string() }
    };

    let chat = Paragraph::new(messages).scroll((scroll_pos, 0))
        .block(Block::default().title(Span::styled(title, Style::default().fg(cb).bold()))
            .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(cb))
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
        .borders(Borders::ALL).border_type(BorderType::Rounded)
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
            Focus::AgentChat => "▌Chat▐",
            _ => "▌Info▐",
        }, Style::default().fg(t.accent).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
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
        ("Latency", match a.latency_ms { Some(ms) => format!("{}ms", ms), None => "—".into() },
            match a.latency_ms { Some(ms) if ms < 100 => t.status_online, Some(ms) if ms < 500 => t.status_busy, Some(_) => t.status_offline, _ => t.text_dim }),
        ("Tokens Today", format!("{}", a.token_burn), t.text),
        ("Last Seen", a.last_seen.clone(), t.text),
        ("Task", a.current_task.as_deref().unwrap_or("none").to_string(), t.text_dim),
    ];

    let info: Vec<Line> = rows.iter().map(|(l, v, c)| Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<14}", l), Style::default().fg(t.text_bold).bold()),
        Span::styled(v.clone(), Style::default().fg(*c)),
    ])).collect();

    let detail = Paragraph::new(info).block(Block::default()
        .title(Span::styled(" Info ", Style::default().fg(ib).bold()))
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(ib))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 1, 0)));
    frame.render_widget(detail, body[0]);

    // Store hit zones
    app.detail_info_area = body[0];
    app.detail_chat_area = body[1];

    // Agent chat
    render_chat_panel(frame, app, body[1], app.focus == Focus::AgentChat, true);

    // Footer
    render_footer(frame, app, chunks[2]);
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
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
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
    .block(Block::default().title(Span::styled(" Mesh Nodes ", Style::default().fg(t.border_active).bold()))
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.border_active))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, outer[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("Esc=back │ Headscale at vpn.tinyblue.dev │ v=VPN │ q=quit", Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, outer[2]);
}


fn render_task_board(frame: &mut Frame, app: &App) {
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
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
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
        let bg = if sel { t.selected_bg } else { app.bg_density.bg() };
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

        let desc: String = task.description.chars().take(30).collect();

        Row::new(vec![
            Cell::from(format!("{}{}", cursor, task.id)),
            Cell::from(format!("{}", task.priority)).style(Style::default().fg(pri_color).bold()),
            Cell::from(format!("{} {}", st_icon, task.status)).style(Style::default().fg(st_color)),
            Cell::from(task.assigned_agent.as_deref().unwrap_or("—").to_string()).style(Style::default().fg(t.accent2)),
            Cell::from(desc).style(Style::default().fg(t.text)),
        ]).style(Style::default().bg(bg)).height(1)
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(5), Constraint::Length(3), Constraint::Length(14),
        Constraint::Length(14), Constraint::Min(15),
    ]).header(hrow)
    .block(Block::default().title(Span::styled(" Tasks ", Style::default().fg(t.border_active).bold()))
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.border_active))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, body[0]);

    // Task detail (right side)
    let detail_lines = if let Some(task) = app.tasks.get(app.task_selected) {
        let st_color = match task.status.as_str() {
            "completed" => t.status_online, "failed" => t.status_offline,
            "running" => t.status_busy, _ => t.text,
        };
        vec![
            Line::from(vec![
                Span::styled("  ID          ", Style::default().fg(t.text_bold).bold()),
                Span::styled(format!("#{}", task.id), Style::default().fg(t.accent)),
            ]),
            Line::from(vec![
                Span::styled("  Priority    ", Style::default().fg(t.text_bold).bold()),
                Span::styled(format!("{}", task.priority), Style::default().fg(t.text)),
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
        .block(Block::default().title(Span::styled(" Detail ", Style::default().fg(t.border).bold()))
            .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.border))
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
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ib))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(input, outer[2]);

    // Footer
    let footer_msg = format!("v0.9 │ t=tasks │ n=new │ d=done │ j/k=navigate │ Esc=back │ {}/{}",
        app.theme_name.label(), app.bg_density.label());
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(footer_msg, Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(footer, outer[3]);
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
        ("  Space", "Toggle agent selection"),
        ("  A (Shift)", "Select all agents"),
        ("  N (Shift)", "Clear selection"),
        ("  a", "New agent wizard"),
        ("  /", "Fleet command (runs on all agents)"),
        ("  o", "OpenClaw version audit"),
        ("  u", "Bulk update OpenClaw"),
        ("  g", "Restart gateway (selected agent)"),
        ("  c", "Cycle color theme"),
        ("  b", "Cycle background density"),
        ("  q", "Quit"),
        ("", ""),
        ("AGENT DETAIL", ""),
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
        ("THEMES (8)", "standard noir paper 1977 2077 matrix sunset arctic"),
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
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(app.bg_density.bg()))
        .padding(Padding::new(2, 2, 1, 1)));
    frame.render_widget(help, frame.area());
}


fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(&app.status_message, Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
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
            app.update_status_bar();
        }

        terminal.draw(|f| {
            match app.screen {
                Screen::Dashboard => render_dashboard(f, &mut app),
                Screen::AgentDetail => render_detail(f, &mut app),
                Screen::TaskBoard => render_task_board(f, &app),
                Screen::VpnStatus => render_vpn_status(f, &app),
                Screen::Help => render_help(f, &app),
            }
            if app.wizard.active {
                wizard::render_wizard(f, &app.wizard, &app.theme, app.bg_density.bg());
            }
        })?;

        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;

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
                                        .args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
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
                    match app.screen {
                        Screen::Help => { app.screen = Screen::Dashboard; }
                        Screen::AgentDetail => match app.focus {
                            Focus::AgentChat => match key.code {
                                KeyCode::Esc => app.focus = Focus::Fleet,
                                KeyCode::Tab => app.focus = Focus::Fleet,
                                KeyCode::Enter => app.send_agent_message().await,
                                KeyCode::Backspace => { app.agent_chat_input.pop(); }
                                KeyCode::Char(c) => app.agent_chat_input.push(c),
                                KeyCode::PageUp => app.agent_chat_scroll = app.agent_chat_scroll.saturating_add(5),
                                KeyCode::PageDown => app.agent_chat_scroll = app.agent_chat_scroll.saturating_sub(5),
                                _ => {}
                            },
                            _ => match key.code {
                                KeyCode::Esc => { app.screen = Screen::Dashboard; app.focus = Focus::Fleet; }
                                KeyCode::Tab => app.focus = Focus::AgentChat,
                                KeyCode::Char('q') => app.should_quit = true,
                                KeyCode::Char('r') => app.start_refresh(),
                                KeyCode::Char('b') => app.cycle_bg(),
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
                                                    let is_mac = host.contains("10.64.0.1") && !host.ends_with(".1");
                                                    let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                                    tokio::time::timeout(
                                                        std::time::Duration::from_secs(8),
                                                        tokio::process::Command::new("ssh")
                                                            .args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
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
                                                let _ = db::create_task(pool, &desc, 5, &app.user(), None).await;
                                                if let Ok(tasks) = db::load_tasks(pool, 50).await { app.tasks = tasks; }
                                            }
                                        }
                                    }
                                    KeyCode::Backspace => { app.task_input.pop(); }
                                    KeyCode::Char(ch) => app.task_input.push(ch),
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc => { app.screen = Screen::Dashboard; app.focus = Focus::Fleet; }
                                    KeyCode::Char('q') => app.should_quit = true,
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
                                    KeyCode::Char('b') => app.cycle_bg(),
                                    KeyCode::Char('c') => app.cycle_theme(),
                                    _ => {}
                                }
                            }
                        }
                        Screen::Dashboard => match app.focus {
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
                                KeyCode::Char('?') => app.screen = Screen::Help,
                                KeyCode::Char('r') => app.start_refresh(),
                                KeyCode::Char('b') => app.cycle_bg(),
                                KeyCode::Char('c') => app.cycle_theme(),
                                KeyCode::Char('s') => app.cycle_sort(),
                                KeyCode::Char('a') => { app.wizard.open(); }
                                KeyCode::Char('A') => {
                                    // Select all
                                    for i in 0..app.agents.len() { app.multi_selected.insert(i); }
                                    app.status_message = format!("Selected all {} agents", app.agents.len());
                                }
                                KeyCode::Char('N') => {
                                    app.multi_selected.clear();
                                    app.status_message = "Selection cleared".into();
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
                                                    .args(["-o","ConnectTimeout=5","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                        &format!("{}@{}", user, host), &cmd])
                                                    .output()
                                            ).await;
                                        });
                                    }
                                    app.status_message = "🔄 OC update dispatched to all agents (background)".into();
                                }
                                KeyCode::Char('g') => {
                                    // Restart gateway on focused agent
                                    if let Some(agent) = app.agents.get(app.selected) {
                                        let host = agent.host.clone();
                                        let user = agent.ssh_user.clone();
                                        let name = agent.name.clone();
                                        let is_mac = agent.os.to_lowercase().contains("mac");
                                        app.status_message = format!("🔄 Restarting gateway on {}...", name);
                                        tokio::spawn(async move {
                                            let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                                            let cmd = format!("{}openclaw gateway restart 2>&1 | tail -1", pfx);
                                            let _ = tokio::process::Command::new("ssh")
                                                .args(["-o","ConnectTimeout=5","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                    &format!("{}@{}", user, host), &cmd])
                                                .output().await;
                                        });
                                    }
                                }
                                KeyCode::Char('v') => {
                                    app.screen = Screen::VpnStatus;
                                }
                                KeyCode::Char('t') => {
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
                                                                .args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
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
                            Focus::Chat => match key.code {
                                KeyCode::Tab | KeyCode::Esc => app.focus = Focus::Fleet,
                                KeyCode::Enter => app.send_message().await,
                                KeyCode::Backspace => { app.chat_input.pop(); }
                                KeyCode::Char(c) => app.chat_input.push(c),
                                KeyCode::PageUp => app.chat_scroll = app.chat_scroll.saturating_add(5),
                                KeyCode::PageDown => app.chat_scroll = app.chat_scroll.saturating_sub(5),
                                _ => {}
                            },
                            _ => {}
                        },
                    }
                }
                    }
            }
        }

        // Auto-refresh every 30s (non-blocking)
        if app.last_refresh.elapsed() > Duration::from_secs(30) && !app.refreshing {
            app.start_refresh();
        }

        // Poll tasks every 5s when on task board
        if app.screen == Screen::TaskBoard && app.last_task_poll.elapsed() > Duration::from_secs(5) {
            if let Some(pool) = &app.db_pool {
                if let Ok(tasks) = db::load_tasks(pool, 50).await { app.tasks = tasks; }
            }
            app.last_task_poll = Instant::now();
        }

        // Poll chat every 3s — spawn so it doesn't block
        if app.last_chat_poll.elapsed() > Duration::from_secs(3) {
            app.poll_chat().await;
            app.last_chat_poll = Instant::now();
        }

        if app.should_quit { break; }
    }

    if let Some(pool) = app.db_pool.take() { pool.disconnect().await?; }
    disable_raw_mode()?;
    stdout().execute(crossterm::event::DisableMouseCapture)?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
