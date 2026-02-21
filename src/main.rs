mod config;
mod db;
mod theme;

use dotenvy;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, MouseEvent, MouseEventKind, MouseButton},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    prelude::*,
    widgets::*,
};
use serde::{Deserialize, Serialize};
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
enum Focus { Fleet, Chat, AgentChat }

#[derive(PartialEq)]
enum Screen { Dashboard, AgentDetail, Help }

struct ProbeResult {
    index: usize,
    status: AgentStatus,
    os: String,
    kernel: String,
    oc_version: String,
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
    // Layout hit zones (updated each frame)
    fleet_area: Rect,
    chat_area: Rect,
    detail_info_area: Rect,
    detail_chat_area: Rect,
    fleet_row_start_y: u16,  // Y offset where first agent row starts
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
                let (status, os, kern, oc) = probe_agent(&host, &user, &sip).await;
                let _ = tx.send(ProbeResult { index: i, status, os, kernel: kern, oc_version: oc });
            });
        }
    }

    fn drain_refresh_results(&mut self) -> Vec<(usize, AgentStatus, String, String, String)> {
        let mut updates = vec![];
        if let Some(rx) = &mut self.refresh_rx {
            while let Ok(r) = rx.try_recv() {
                if r.index < self.agents.len() {
                    self.agents[r.index].status = r.status.clone();
                    if !r.os.is_empty() { self.agents[r.index].os = r.os.clone(); }
                    if !r.kernel.is_empty() { self.agents[r.index].kernel = r.kernel.clone(); }
                    if !r.oc_version.is_empty() { self.agents[r.index].oc_version = r.oc_version.clone(); }
                    self.agents[r.index].last_seen = now_str();
                    updates.push((r.index, r.status, r.os, r.kernel, r.oc_version));
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
        let refresh = if self.refreshing { " ⟳" } else { "" };
        self.status_message = format!(
            "v0.8 │ {}/{} online{} │ theme:{}/{} │ r=refresh b=bg c=color ?=help",
            on, total, refresh, self.theme_name.label(), self.bg_density.label()
        );
    }
}

// ---- SSH Probe ----

async fn probe_agent(host: &str, user: &str, self_ip: &str) -> (AgentStatus, String, String, String) {
    if host == "localhost" || host == self_ip {
        let os = Command::new("bash").args(["-c", ". /etc/os-release 2>/dev/null && echo \"$NAME $VERSION_ID\" || echo unknown"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let kern = Command::new("uname").arg("-r").output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let oc = Command::new("bash").args(["-c", "openclaw --version 2>/dev/null || echo ?"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        return (AgentStatus::Online, os, kern, oc);
    }
    let tgt = format!("{}@{}", user, host);
    let script = r#"OS=$(. /etc/os-release 2>/dev/null && echo "$NAME $VERSION_ID" || (sw_vers -productName 2>/dev/null; sw_vers -productVersion 2>/dev/null) || echo ?); KERN=$(uname -r); OC=$(openclaw --version 2>/dev/null || echo ?); echo "OS:$OS"; echo "KERN:$KERN"; echo "OC:$OC""#;
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        Command::new("ssh").args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",&tgt,"bash","-c",script]).output()
    ).await;
    let result = match result {
        Ok(r) => r,
        Err(_) => return (AgentStatus::Offline, String::new(), String::new(), String::new()),
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
            (AgentStatus::Online, os, kern, oc)
        }
        _ => (AgentStatus::Offline, String::new(), String::new(), String::new()),
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

fn render_dashboard(frame: &mut Frame, app: &mut App) {
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

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[1]);

    app.fleet_area = body[0];
    app.chat_area = body[1];
    render_fleet_table(frame, app, body[0], app.focus == Focus::Fleet);
    render_chat_panel(frame, app, body[1], app.focus == Focus::Chat, false);
    render_footer(frame, app, outer[2]);
}

fn render_fleet_table(frame: &mut Frame, app: &mut App, area: Rect, active: bool) {
    let t = &app.theme;
    let fb = if active { t.border_active } else { t.border };

    let hcells = ["  ", "Agent", "Location", "Status", "Version"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
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
        Row::new(vec![
            Cell::from(format!(" {}", a.emoji)),
            Cell::from(a.name.clone()).style(Style::default().fg(t.text_bold).bold()),
            Cell::from(a.location.clone()).style(Style::default().fg(loc_color)),
            Cell::from(a.status.to_string()).style(Style::default().fg(st_color)),
            Cell::from(a.oc_version.clone()).style(Style::default().fg(t.version)),
        ]).style(Style::default().bg(bg)).height(1)
    }).collect();

    app.fleet_row_start_y = area.y + 1; // +1 for border, +1 for header handled in click calc

    let table = Table::new(rows, [
        Constraint::Length(4), Constraint::Length(14), Constraint::Length(9),
        Constraint::Length(12), Constraint::Min(12),
    ]).header(hrow)
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
        " Chat ".to_string()
    };

    let chat = Paragraph::new(messages).scroll((scroll_pos, 0))
        .block(Block::default().title(Span::styled(title, Style::default().fg(cb).bold()))
            .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(cb))
            .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(chat, cl[0]);

    let prompt = if agent_mode {
        format!(" @{} › ", app.agents[app.selected].db_name)
    } else if active {
        " broadcast to all ⏎ ".to_string()
    } else {
        " Tab to chat ".to_string()
    };

    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(t.accent)),
        Span::styled(input_text.as_str(), Style::default().fg(t.text)),
        if active { Span::styled("▌", Style::default().fg(t.accent)) } else { Span::raw("") },
    ])).block(Block::default().title(prompt)
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if active { t.border_active } else { t.border }))
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

    // Body: info left, chat right
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
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

fn render_help(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let sections = vec![
        ("", ""),
        ("NAVIGATION", ""),
        ("  Tab", "Switch focus (Fleet ↔ Chat / Info ↔ Agent Chat)"),
        ("  ↑↓ / j k", "Navigate fleet list"),
        ("  Enter", "Open agent detail with dedicated chat"),
        ("  Esc", "Back to dashboard"),
        ("  r", "Refresh all agents via SSH (non-blocking)"),
        ("  q", "Quit"),
        ("", ""),
        ("THEMES", ""),
        ("  b", "Cycle background: dark → medium → light → white → terminal"),
        ("  c", "Cycle colors: standard → noir → paper → 1977 → 2077 → matrix → sunset → arctic"),
        ("", ""),
        ("CHAT", ""),
        ("  @agent msg", "Message a specific agent (dashboard)"),
        ("  Type + Enter", "Send to focused agent (detail screen)"),
        ("  PgUp/PgDn", "Scroll chat"),
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

    let help = Paragraph::new(lines).block(Block::default().title(" Help — press any key to close ")
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
    if dotenvy::dotenv().is_err() {
        if let Ok(home) = std::env::var("HOME") {
            let _ = dotenvy::from_path(std::path::Path::new(&home).join(".config/sam/.env"));
        }
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
                for (idx, _status, _os, _kern, _oc) in &updates {
                    let a = &app.agents[*idx];
                    let p = pool.clone();
                    let (name, st, os, kern, oc) = (
                        a.db_name.clone(), a.status.to_db_str().to_string(),
                        if a.os.is_empty() { None } else { Some(a.os.clone()) },
                        if a.kernel.is_empty() { None } else { Some(a.kernel.clone()) },
                        if a.oc_version.is_empty() { None } else { Some(a.oc_version.clone()) },
                    );
                    tokio::spawn(async move {
                        let _ = db::update_agent_status(&p, &name, &st,
                            os.as_deref(), kern.as_deref(), oc.as_deref()).await;
                    });
                }
            }
            app.update_status_bar();
        }

        terminal.draw(|f| match app.screen {
            Screen::Dashboard => render_dashboard(f, &mut app),
            Screen::AgentDetail => render_detail(f, &mut app),
            Screen::Help => render_help(f, &app),
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
                                _ => {}
                            },
                        },
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
                                KeyCode::Char('?') => app.screen = Screen::Help,
                                KeyCode::Char('r') => app.start_refresh(),
                                KeyCode::Char('b') => app.cycle_bg(),
                                KeyCode::Char('c') => app.cycle_theme(),
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

        // Auto-refresh every 30s (non-blocking)
        if app.last_refresh.elapsed() > Duration::from_secs(30) && !app.refreshing {
            app.start_refresh();
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
