mod config;
mod db;

use dotenvy;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
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
}

#[derive(PartialEq, Clone)]
enum Focus { Fleet, Chat }

#[derive(PartialEq)]
enum Screen { Dashboard, AgentDetail, Help }

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
    self_ip: String,
}

impl App {
    async fn new(fleet_config: config::FleetConfig) -> Self {
        let pool = db::get_pool();
        let self_ip = std::env::var("SAM_SELF_IP").unwrap_or_else(|_| "localhost".into());
        let mut agents = Vec::new();

        // Load from DB, enriched with fleet.toml metadata
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

        let chat_history = match db::load_chat_history(&pool, 100).await {
            Ok(msgs) => msgs.iter().map(|m| ChatLine {
                sender: m.sender.clone(), target: m.target.clone(),
                message: m.message.clone(), response: m.response.clone(),
                time: m.created_at.clone(), status: m.status.clone(),
            }).collect(),
            Err(_) => vec![],
        };

        App {
            fleet_config: fleet_config.agent,
            agents, selected: 0, screen: Screen::Dashboard, focus: Focus::Fleet,
            should_quit: false, last_refresh: Instant::now(), last_chat_poll: Instant::now(),
            status_message: "v0.6 │ Tab=switch focus │ ?=help".into(),
            db_pool: Some(pool), chat_input: String::new(), chat_history, chat_scroll: 0,
            self_ip,
        }
    }

    fn next(&mut self) { if self.selected < self.agents.len() - 1 { self.selected += 1; } }
    fn previous(&mut self) { if self.selected > 0 { self.selected -= 1; } }

    async fn send_message(&mut self) {
        if self.chat_input.trim().is_empty() { return; }
        let input = self.chat_input.clone();
        self.chat_input.clear();

        let (target, message) = if input.trim().starts_with('@') {
            if let Some(pos) = input.trim().find(' ') {
                let raw = &input.trim()[1..pos];
                let resolved = config::resolve_alias(raw, &self.fleet_config);
                (Some(resolved), input.trim()[pos+1..].trim().to_string())
            } else { (None, input.trim().to_string()) }
        } else { (None, input.trim().to_string()) };

        self.chat_history.push(ChatLine {
            sender: std::env::var("SAM_USER").unwrap_or_else(|_| "operator".into()), target: target.clone(), message: message.clone(),
            response: None, time: now_str(), status: "pending".into(),
        });

        if let Some(pool) = &self.db_pool {
            let _ = db::send_chat(pool, &std::env::var("SAM_USER").unwrap_or_else(|_| "operator".into()), target.as_deref(), &message).await;
        }
        self.chat_scroll = 0;
    }

    async fn poll_chat(&mut self) {
        if let Some(pool) = &self.db_pool {
            if let Ok(msgs) = db::load_chat_history(pool, 100).await {
                self.chat_history = msgs.iter().map(|m| ChatLine {
                    sender: m.sender.clone(), target: m.target.clone(),
                    message: m.message.clone(), response: m.response.clone(),
                    time: m.created_at.clone(), status: m.status.clone(),
                }).collect();
            }
        }
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
    let result = Command::new("ssh").args(["-o","ConnectTimeout=4","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",&tgt,"bash","-c",script]).output().await;
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

async fn refresh_fleet(agents: &mut Vec<Agent>, pool: &Option<mysql_async::Pool>, self_ip: &str) {
    let mut handles = vec![];
    for a in agents.iter() {
        let (h, u, sip) = (a.host.clone(), a.ssh_user.clone(), self_ip.to_string());
        handles.push(tokio::spawn(async move { probe_agent(&h, &u, &sip).await }));
    }
    for (i, handle) in handles.into_iter().enumerate() {
        if let Ok((status, os, kern, oc)) = handle.await {
            agents[i].status = status;
            if !os.is_empty() { agents[i].os = os; }
            if !kern.is_empty() { agents[i].kernel = kern; }
            if !oc.is_empty() { agents[i].oc_version = oc; }
            agents[i].last_seen = now_str();
            if let Some(p) = pool {
                let _ = db::update_agent_status(p, &agents[i].db_name, agents[i].status.to_db_str(),
                    if agents[i].os.is_empty() { None } else { Some(&agents[i].os) },
                    if agents[i].kernel.is_empty() { None } else { Some(&agents[i].kernel) },
                    if agents[i].oc_version.is_empty() { None } else { Some(&agents[i].oc_version) },
                ).await;
            }
        }
    }
}

fn now_str() -> String {
    use std::process::Command as C;
    C::new("date").arg("+%H:%M:%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("now".into())
}

// ---- Colors ----

fn status_style(s: &AgentStatus) -> Style {
    Style::default().fg(match s {
        AgentStatus::Online => Color::Green, AgentStatus::Busy => Color::Yellow,
        AgentStatus::Offline => Color::Red, AgentStatus::Probing => Color::Blue,
        AgentStatus::Unknown => Color::DarkGray,
    })
}

fn loc_style(l: &str) -> Style {
    Style::default().fg(match l {
        "Home" => Color::Rgb(100, 200, 100), "SM" => Color::Rgb(230, 180, 60),
        "VPS" => Color::Rgb(180, 120, 220), "Mobile" => Color::Rgb(80, 180, 230),
        _ => Color::White,
    })
}

// ---- Rendering ----

fn render_dashboard(frame: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    let online = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
    let total = app.agents.len();
    let live = app.last_refresh.elapsed() < Duration::from_secs(60);

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🛰️  S.A.M MISSION CONTROL", Style::default().fg(Color::Rgb(80, 200, 255)).bold()),
        Span::raw("    "),
        Span::styled(format!("{}", online), Style::default().fg(Color::Green).bold()),
        Span::styled(format!("/{} agents", total), Style::default().fg(Color::DarkGray)),
        Span::raw("    "),
        Span::styled(if live { "● live" } else { "○ stale" }, Style::default().fg(if live { Color::Green } else { Color::Red })),
        Span::raw("    "),
        Span::styled(match app.focus { Focus::Fleet => "▌Fleet▐", Focus::Chat => "▌Chat▐" }, Style::default().fg(Color::Rgb(80, 200, 255)).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Rgb(60, 60, 80))));
    frame.render_widget(header, outer[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[1]);

    // Fleet
    let fleet_active = app.focus == Focus::Fleet;
    let fb = if fleet_active { Color::Rgb(80, 200, 255) } else { Color::Rgb(50, 50, 70) };

    let hcells = ["  ", "Agent", "Location", "Status", "Version"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(Color::Rgb(180, 180, 200)).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.agents.iter().enumerate().map(|(i, a)| {
        let sel = i == app.selected && fleet_active;
        let bg = if sel { Color::Rgb(35, 40, 60) } else { Color::Reset };
        Row::new(vec![
            Cell::from(format!(" {}", a.emoji)),
            Cell::from(a.name.clone()).style(Style::default().fg(Color::White).bold()),
            Cell::from(a.location.clone()).style(loc_style(&a.location)),
            Cell::from(a.status.to_string()).style(status_style(&a.status)),
            Cell::from(a.oc_version.clone()).style(Style::default().fg(Color::Rgb(120, 200, 220))),
        ]).style(Style::default().bg(bg)).height(1)
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(4), Constraint::Length(14), Constraint::Length(9),
        Constraint::Length(12), Constraint::Min(12),
    ]).header(hrow)
    .block(Block::default().title(Span::styled(" Fleet ", Style::default().fg(fb).bold()))
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(fb))
        .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(table, body[0]);

    // Chat
    let chat_active = app.focus == Focus::Chat;
    let cb = if chat_active { Color::Rgb(80, 200, 255) } else { Color::Rgb(50, 50, 70) };

    let cl = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(body[1]);

    let mut lines: Vec<Line> = Vec::new();
    if app.chat_history.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Type @agent message and press Enter", Style::default().fg(Color::Rgb(80, 80, 100)))));
        lines.push(Line::from(Span::styled("  Example: @nix check disk space", Style::default().fg(Color::Rgb(60, 60, 80)))));
    } else {
        for msg in &app.chat_history {
            let ts = msg.target.as_ref().map(|t| format!("→@{}", t)).unwrap_or_else(|| "→all".into());
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", msg.time), Style::default().fg(Color::Rgb(80, 80, 100))),
                Span::styled(&msg.sender, Style::default().fg(if msg.sender == std::env::var("SAM_USER").unwrap_or_else(|_| "operator".into()) { Color::Rgb(230, 180, 60) } else { Color::Rgb(100, 220, 100) }).bold()),
                Span::styled(format!(" {}", ts), Style::default().fg(Color::Rgb(80, 80, 120))),
            ]));
            lines.push(Line::from(vec![
                Span::raw("     "),
                Span::styled(&msg.message, Style::default().fg(Color::Rgb(220, 220, 230))),
            ]));
            if let Some(resp) = &msg.response {
                let max_w = 50;
                let words: Vec<&str> = resp.split_whitespace().collect();
                let (mut cur, mut first) = (String::new(), true);
                for w in words {
                    if cur.len() + w.len() + 1 > max_w && !cur.is_empty() {
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            if first { Span::styled("↳ ", Style::default().fg(Color::Rgb(100, 200, 100))) } else { Span::raw("  ") },
                            Span::styled(cur.clone(), Style::default().fg(Color::Rgb(160, 210, 170))),
                        ]));
                        cur.clear(); first = false;
                    }
                    if !cur.is_empty() { cur.push(' '); }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        if first { Span::styled("↳ ", Style::default().fg(Color::Rgb(100, 200, 100))) } else { Span::raw("  ") },
                        Span::styled(cur, Style::default().fg(Color::Rgb(160, 210, 170))),
                    ]));
                }
            } else if msg.status == "pending" {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled("⏳ awaiting response...", Style::default().fg(Color::Rgb(100, 100, 120))),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    let vh = cl[0].height.saturating_sub(2) as usize;
    let tl = lines.len();
    let scroll = if tl > vh && app.chat_scroll == 0 { (tl - vh) as u16 } else { app.chat_scroll };

    let chat = Paragraph::new(lines).scroll((scroll, 0))
        .block(Block::default().title(Span::styled(" Chat ", Style::default().fg(cb).bold()))
            .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(cb)));
    frame.render_widget(chat, cl[0]);

    let ib = if chat_active { Color::Rgb(80, 200, 255) } else { Color::Rgb(50, 50, 70) };
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(&app.chat_input, Style::default().fg(Color::White)),
        if chat_active { Span::styled("▌", Style::default().fg(Color::Rgb(80, 200, 255))) } else { Span::raw("") },
    ])).block(Block::default()
        .title(if chat_active { " @agent message ⏎ " } else { " Tab to chat " })
        .borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(ib)));
    frame.render_widget(input, cl[1]);

    // Footer
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(&app.status_message, Style::default().fg(Color::Rgb(140, 140, 160))),
        Span::raw("    "),
        Span::styled("Tab", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Focus  ", Style::default().fg(Color::Rgb(100, 100, 120))),
        Span::styled("r", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Refresh  ", Style::default().fg(Color::Rgb(100, 100, 120))),
        Span::styled("⏎", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Detail  ", Style::default().fg(Color::Rgb(100, 100, 120))),
        Span::styled("?", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Help  ", Style::default().fg(Color::Rgb(100, 100, 120))),
        Span::styled("q", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Quit", Style::default().fg(Color::Rgb(100, 100, 120))),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Rgb(40, 40, 60))));
    frame.render_widget(footer, outer[2]);
}

fn render_detail(frame: &mut Frame, app: &App) {
    let a = &app.agents[app.selected];
    let chunks = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    let header = Paragraph::new(format!("  {} {}  —  Agent Detail", a.emoji, a.name))
        .style(Style::default().fg(Color::Rgb(80, 200, 255)).bold())
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Rgb(60, 60, 80))));
    frame.render_widget(header, chunks[0]);

    let caps = if a.capabilities.is_empty() { "none".into() } else { a.capabilities.join(", ") };
    let rows = vec![
        ("Host", a.host.clone(), Color::White),
        ("Location", a.location.clone(), loc_style(&a.location).fg.unwrap_or(Color::White)),
        ("Status", a.status.to_string(), status_style(&a.status).fg.unwrap_or(Color::White)),
        ("OS", a.os.clone(), Color::White),
        ("Kernel", a.kernel.clone(), Color::White),
        ("Version", a.oc_version.clone(), Color::Rgb(120, 200, 220)),
        ("SSH User", a.ssh_user.clone(), Color::White),
        ("Capabilities", caps, Color::White),
        ("Tokens Today", format!("{}", a.token_burn), Color::White),
        ("Last Seen", a.last_seen.clone(), Color::White),
        ("Task", a.current_task.as_deref().unwrap_or("none").to_string(), Color::DarkGray),
    ];

    let info: Vec<Line> = rows.iter().map(|(l, v, c)| Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<16}", l), Style::default().fg(Color::Rgb(180, 180, 200)).bold()),
        Span::styled(v, Style::default().fg(*c)),
    ])).collect();

    let detail = Paragraph::new(info).block(Block::default().title(" Detail ").borders(Borders::ALL)
        .border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Rgb(60, 60, 80)))
        .padding(Padding::new(1, 1, 1, 0)));
    frame.render_widget(detail, chunks[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("Esc", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Back  ", Style::default().fg(Color::Rgb(100, 100, 120))),
        Span::styled("q", Style::default().fg(Color::Rgb(80, 200, 255))),
        Span::styled(" Quit", Style::default().fg(Color::Rgb(100, 100, 120))),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Rgb(40, 40, 60))));
    frame.render_widget(footer, chunks[2]);
}

fn render_help(frame: &mut Frame) {
    let sections = vec![
        ("", ""),
        ("NAVIGATION", ""),
        ("  Tab", "Switch focus between Fleet and Chat"),
        ("  ↑↓ / j k", "Navigate fleet list"),
        ("  Enter", "Open agent detail"),
        ("  Esc", "Back / unfocus chat"),
        ("  r", "Refresh all agents via SSH"),
        ("  q", "Quit"),
        ("", ""),
        ("CHAT", ""),
        ("  @agent msg", "Message a specific agent"),
        ("  msg", "Broadcast to all"),
        ("  PgUp/PgDn", "Scroll chat"),
        ("", ""),
        ("CONFIG", ""),
        ("  fleet.toml", "Fleet agent definitions"),
        ("  .env", "Database credentials"),
        ("", "Agents resolve by name, display, or prefix match"),
    ];

    let lines: Vec<Line> = sections.iter().map(|(l, r)| {
        if r.is_empty() && !l.is_empty() && !l.starts_with(' ') {
            Line::from(Span::styled(format!("  {}", l), Style::default().fg(Color::Rgb(80, 200, 255)).bold()))
        } else {
            Line::from(vec![
                Span::styled(format!("  {:<14}", l), Style::default().fg(Color::Rgb(230, 180, 60))),
                Span::styled(*r, Style::default().fg(Color::Rgb(180, 180, 200))),
            ])
        }
    }).collect();

    let help = Paragraph::new(lines).block(Block::default().title(" Help — press any key to close ")
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(80, 200, 255)))
        .padding(Padding::new(2, 2, 1, 1)));
    frame.render_widget(help, frame.area());
}

// ---- Main ----

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env from current dir, then ~/.config/sam/
    if dotenvy::dotenv().is_err() {
        if let Ok(home) = std::env::var("HOME") {
            let _ = dotenvy::from_path(std::path::Path::new(&home).join(".config/sam/.env"));
        }
    }

    // Load fleet config
    let fleet_config = match config::load_fleet_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = App::new(fleet_config).await;
    for a in &mut app.agents { a.status = AgentStatus::Probing; }
    app.status_message = "Probing fleet...".into();
    terminal.draw(|f| render_dashboard(f, &app))?;

    let sip = app.self_ip.clone();
    refresh_fleet(&mut app.agents, &app.db_pool, &sip).await;
    app.last_refresh = Instant::now();
    let on = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
    app.status_message = format!("v0.6 │ {}/{} online │ Tab=focus ?=help", on, app.agents.len());

    loop {
        terminal.draw(|f| match app.screen {
            Screen::Dashboard => render_dashboard(f, &app),
            Screen::AgentDetail => render_detail(f, &app),
            Screen::Help => render_help(f),
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.screen {
                        Screen::Help => { app.screen = Screen::Dashboard; }
                        Screen::AgentDetail => match key.code {
                            KeyCode::Esc => app.screen = Screen::Dashboard,
                            KeyCode::Char('q') => app.should_quit = true,
                            _ => {}
                        },
                        Screen::Dashboard => match app.focus {
                            Focus::Fleet => match key.code {
                                KeyCode::Char('q') => app.should_quit = true,
                                KeyCode::Tab => app.focus = Focus::Chat,
                                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                                KeyCode::Down | KeyCode::Char('j') => app.next(),
                                KeyCode::Enter => app.screen = Screen::AgentDetail,
                                KeyCode::Char('?') => app.screen = Screen::Help,
                                KeyCode::Char('r') => {
                                    app.status_message = "Refreshing...".into();
                                    terminal.draw(|f| render_dashboard(f, &app))?;
                                    let sip = app.self_ip.clone();
                                    refresh_fleet(&mut app.agents, &app.db_pool, &sip).await;
                                    app.last_refresh = Instant::now();
                                    let on = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
                                    app.status_message = format!("v0.6 │ {}/{} online │ Tab=focus ?=help", on, app.agents.len());
                                }
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
                        },
                    }
                }
            }
        }

        if app.last_refresh.elapsed() > Duration::from_secs(30) {
            let sip = app.self_ip.clone();
            refresh_fleet(&mut app.agents, &app.db_pool, &sip).await;
            app.last_refresh = Instant::now();
            let on = app.agents.iter().filter(|a| a.status == AgentStatus::Online).count();
            app.status_message = format!("v0.6 │ {}/{} online │ Tab=focus ?=help", on, app.agents.len());
        }

        if app.last_chat_poll.elapsed() > Duration::from_secs(3) {
            app.poll_chat().await;
            app.last_chat_poll = Instant::now();
        }

        if app.should_quit { break; }
    }

    if let Some(pool) = app.db_pool.take() { pool.disconnect().await?; }
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
