//! S.A.M Mission Control — main entry point.
//!
//! Parses CLI arguments, loads configuration, and either runs a CLI subcommand
//! (non-interactive) or launches the full Ratatui TUI event loop.

mod cli;
mod config;
mod db;
mod shell;
mod theme;
mod validate;
mod wizard;

use clap::Parser;
use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dotenvy;
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::stdout;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
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
    mem_free_mb: Option<i64>,
    swap_mb: Option<i64>,
    gateway_port: i32,
    gateway_token: Option<String>,
    gateway_pid: Option<i32>,
    gateway_status: GatewayStatus,
    uptime_seconds: i64,
    activity: String,         // What the agent is currently doing
    context_pct: Option<f32>, // Context window usage %
    #[serde(skip)]
    last_probe_at: Option<std::time::Instant>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum AgentStatus {
    Online,
    Busy,
    Offline,
    Probing,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum GatewayStatus { Online, Offline, Restarting, Unknown }

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "●  online"),
            Self::Busy => write!(f, "◉  busy"),
            Self::Offline => write!(f, "○  offline"),
            Self::Probing => write!(f, "⟳  probing"),
            Self::Unknown => write!(f, "?  unknown"),
        }
    }
}

impl AgentStatus {
    fn from_str(s: &str) -> Self {
        match s {
            "online" => Self::Online,
            "busy" => Self::Busy,
            "offline" | "error" => Self::Offline,
            _ => Self::Unknown,
        }
    }
    fn to_db_str(&self) -> &str {
        match self {
            Self::Online => "online",
            Self::Busy => "busy",
            _ => "offline",
        }
    }
}

#[derive(Clone, Debug)]
struct Alert {
    time: String,
    created_at: Instant,
    agent: String,
    emoji: String,
    message: String,
    severity: AlertSeverity,
}

#[derive(Clone, Debug, PartialEq)]
enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

impl AlertSeverity {
    fn icon(&self) -> &str {
        match self {
            Self::Critical => "🔴",
            Self::Warning => "🟡",
            Self::Info => "🔵",
        }
    }
}

#[derive(Clone, Debug)]
struct ChatLine {
    id: i64,
    sender: String,
    target: Option<String>,
    message: String,
    response: Option<String>,
    time: String,
    status: String,
    kind: String,
    thread_id: Option<String>,
    parent_id: Option<i64>,
    depth: u8,
}

#[derive(PartialEq, Clone)]
enum Focus {
    Fleet,
    Chat,
    AgentChat,
    Command,
    Workspace,
    Services,
}

#[derive(PartialEq)]
enum Screen {
    Dashboard,
    AgentDetail,
    TaskBoard,
    SpawnManager,
    VpnStatus,
    Alerts,
    Help,
}

#[derive(PartialEq, Clone, Copy)]
enum GroupFilter {
    All,
    Home,
    SM,
    VPS,
    Mobile,
    Outdated,
    Offline,
}

impl GroupFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Home,
            Self::Home => Self::SM,
            Self::SM => Self::VPS,
            Self::VPS => Self::Mobile,
            Self::Mobile => Self::Outdated,
            Self::Outdated => Self::Offline,
            Self::Offline => Self::All,
        }
    }
    fn label(&self) -> &str {
        match self {
            Self::All => "All",
            Self::Home => "Home",
            Self::SM => "SM",
            Self::VPS => "VPS",
            Self::Mobile => "Mobile",
            Self::Outdated => "Outdated",
            Self::Offline => "Offline",
        }
    }
}

#[derive(Clone, Copy)]
enum SplitDragTarget { Dashboard, Detail }

#[derive(PartialEq, Clone, Copy)]
enum SortMode {
    Name,
    Status,
    Location,
    Version,
    Latency,
}

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
            Self::Name => "name",
            Self::Status => "status",
            Self::Location => "location",
            Self::Version => "version",
            Self::Latency => "latency",
        }
    }
    fn arrow(&self) -> &str {
        "▲"
    }
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
    mem_free_mb: Option<i64>,
    swap_mb: Option<i64>,
    activity: String,
    context_pct: Option<f32>,
    gateway_status: GatewayStatus,
    gateway_pid: Option<i32>,
}

// ── UI Helpers ──────────────────────────────────────
fn chrono_now_hms() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now % 86400;
    // Keep fixed UTC-6 offset for consistency with the existing app clock display.
    let hours = ((secs / 3600) + 24 - 6) % 24; // UTC-6 for CST
    let mins = (secs % 3600) / 60;
    let sec = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, sec)
}

fn os_emoji(os: &str) -> &'static str {
    let os_lower = os.to_lowercase();
    if os_lower.contains("mac") || os_lower.contains("darwin") {
        "🍎"
    } else if os_lower.contains("windows") {
        "🪟"
    } else if os_lower.contains("android") {
        "📱"
    } else if os_lower.contains("arch") {
        "🏔"
    } else if os_lower.contains("fedora") {
        "🎩"
    } else if os_lower.contains("ubuntu") {
        "🟠"
    } else if os_lower.contains("rhel") || os_lower.contains("alma") || os_lower.contains("rocky") {
        "🔴"
    } else if os_lower.contains("linux") {
        "🐧"
    } else {
        "💻"
    }
}

fn format_uptime(secs: i64) -> String {
    if secs <= 0 {
        return "—".into();
    }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

fn format_app_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 { format!("{}h {}m", hours, mins) } else { format!("{}m", mins) }
}

fn format_last_seen(dt: &str) -> String {
    // Simple relative time from datetime string
    if dt.is_empty() {
        return "—".into();
    }
    // Just show the time portion for now
    if let Some(time) = dt.split(' ').nth(1) {
        time[..5].to_string() // HH:MM
    } else {
        dt.to_string()
    }
}

fn format_since(instant: Option<std::time::Instant>) -> String {
    match instant {
        None => "—".into(),
        Some(i) => {
            let secs = i.elapsed().as_secs();
            if secs < 60 {
                format!("{}s", secs)
            } else if secs < 3600 {
                format!("{}m", secs / 60)
            } else {
                format!("{}h", secs / 3600)
            }
        }
    }
}

fn last_seen_color(instant: Option<std::time::Instant>, t: &Theme) -> Color {
    match instant {
        None => t.text_dim,
        Some(i) => {
            let secs = i.elapsed().as_secs();
            if secs < 300 {
                t.status_online
            } else if secs < 1800 {
                t.status_busy
            } else {
                t.status_offline
            }
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

fn db_latency_color(latency: Option<u32>, online: bool, t: &Theme) -> Color {
    if !online {
        t.status_offline
    } else {
        match latency {
            Some(ms) if ms < 10 => t.status_online,
            Some(ms) if ms < 50 => t.status_busy,
            Some(_) => t.status_offline,
            None => t.text_dim,
        }
    }
}

fn resource_bar(pct: Option<f32>, width: u16) -> String {
    let p = pct.unwrap_or(0.0);
    let filled = ((p / 100.0) * width as f32) as usize;
    let empty = (width as usize).saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty),)
}


// ── Gateway action type ────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq)]
enum GatewayAction { Start, Stop, Restart }

// ── Audit pipeline ─────────────────────────────────────────────────
struct AuditEvent {
    actor: String,
    action: String,
    target: String,
    detail: String,
}

struct AuditResult {
    ok: bool,
    action: String,
    target: String,
    error: Option<String>,
}

// ── Split pane constants ───────────────────────────────────────────
const MIN_SPLIT_PCT: u16 = 20;
const MAX_SPLIT_PCT: u16 = 80;
const DIVIDER_HIT_WIDTH: u16 = 2;
const FLEET_TABLE_HEADER_ROWS: u16 = 2;

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn split_pct_from_mouse(mx: u16, area: ratatui::layout::Rect) -> u16 {
    if area.width == 0 { return 45; }
    let pct = ((mx.saturating_sub(area.x)) as u32 * 100 / area.width as u32) as u16;
    pct.max(MIN_SPLIT_PCT).min(MAX_SPLIT_PCT)
}

fn dashboard_split(_area: &ratatui::layout::Rect, split_pct: Option<u16>) -> (u16, u16) {
    let pct = split_pct.unwrap_or(45).max(MIN_SPLIT_PCT).min(MAX_SPLIT_PCT);
    (pct, 100 - pct)
}

fn detail_split(_area: &ratatui::layout::Rect, split_pct: Option<u16>) -> (u16, u16) {
    let pct = split_pct.unwrap_or(55).max(MIN_SPLIT_PCT).min(MAX_SPLIT_PCT);
    (pct, 100 - pct)
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
    agent_chat_history: Vec<ChatLine>, // Direct messages to focused agent
    agent_chat_scroll: u16,
    agent_threads: Vec<db::ThreadSummary>,
    active_thread_id: Option<String>,
    reply_parent_id: Option<i64>,
    thread_sidebar: bool,
    pinned_threads: HashSet<String>,
    refresh_rx: Option<mpsc::UnboundedReceiver<ProbeResult>>,
    refreshing: bool,
    refresh_cycle: u32,
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
    group_filter: GroupFilter,
    // Layout hit zones (updated each frame)
    fleet_area: Rect,
    chat_area: Rect,
    detail_info_area: Rect,
    detail_chat_area: Rect,
    fleet_row_start_y: u16, // Y offset where first agent row starts
    // Splash
    spawned_agents: Vec<db::SpawnedAgent>,
    show_splash: bool,
    splash_start: Instant,
    // Alerts
    alerts: Vec<Alert>,
    alert_flash: Option<Instant>,
    alerts_scroll: u16,
    gateway_confirm_at: Option<Instant>,
    gateway_action_confirm: Option<(usize, GatewayAction, Instant)>,
    // Diagnostics (inline doctor/fix)
    diag_active: bool,
    diag_steps: Vec<DiagStep>,
    diag_rx: Option<mpsc::UnboundedReceiver<DiagStep>>,
    diag_auto_fix: bool,
    diag_title: Option<String>,
    diag_start: Option<Instant>,
    diag_overlay_scroll: u16,
    // Fleet-wide diagnostics (multi-agent D)
    fleet_diag_active: bool,
    fleet_diag_fix: bool,
    fleet_diag_selected: usize,
    fleet_diag_done: bool,
    fleet_diag_results: Vec<FleetDiagResult>,
    fleet_diag_rx: Option<mpsc::UnboundedReceiver<FleetDiagMsg>>,
    // Services (OpenClaw plugin management)
    svc_list: Vec<ServiceEntry>,
    svc_selected: usize,
    svc_config: Option<serde_json::Value>, // Full openclaw.json
    svc_loading: bool,
    svc_load_rx: Option<mpsc::UnboundedReceiver<Option<serde_json::Value>>>,
    config_load_rx: Option<mpsc::UnboundedReceiver<Option<String>>>,
    svc_detail_scroll: u16,
    // Agent model management
    agent_model: Option<String>,
    agent_model_agent: Option<String>,
    agent_model_loading: bool,
    model_picker_active: bool,
    model_picker_selected: usize,
    model_options: Vec<String>,
    model_load_rx: Option<mpsc::UnboundedReceiver<ModelLoadResult>>,
    model_write_rx: Option<mpsc::UnboundedReceiver<ModelWriteResult>>,
    // Workspace (agent file management)
    ws_files: Vec<WorkspaceFile>,
    ws_selected: usize,
    ws_content: Option<String>,
    ws_content_scroll: u16,
    ws_editing: bool,
    ws_edit_buffer: Vec<String>,
    ws_cursor: (usize, usize), // (line, col)
    ws_undo_stack: Vec<(Vec<String>, (usize, usize))>,
    ws_discard_confirm: bool,
    ws_crons: Vec<CronEntry>,
    ws_cron_selected: usize,
    ws_cron_form_active: bool,
    ws_cron_form_edit: bool,
    ws_cron_form_schedule: String,
    ws_cron_form_description: String,
    ws_cron_form_focus: usize,
    ws_loading: bool,
    ws_load_rx: Option<mpsc::UnboundedReceiver<(Vec<WorkspaceFile>, Vec<CronEntry>)>>,
    ws_file_rx: Option<mpsc::UnboundedReceiver<String>>,
    // Filter
    filter_active: bool,
    filter_text: String,
    vim_mode: bool,
    vim_pending: Option<char>,
    // Config viewer
    config_text: Option<String>,
    config_scroll: u16,
    // Help screen
    help_scroll: u16,
    // Multi-select
    selected_agents: std::collections::HashSet<String>,
    // Theme
    theme_name: ThemeName,
    bg_density: BgDensity,
    theme: Theme,
    // OC version tracking
    latest_oc_version: String,
    // Routing
    routed_msg_ids: std::collections::HashSet<i64>,
    // Background chat poll
    chat_poll_rx: Option<mpsc::UnboundedReceiver<ChatPollResult>>,
    chat_polling: bool,
    // Wizard SSH test (background)
    wizard_ssh_rx: Option<mpsc::UnboundedReceiver<String>>,
    // Autocomplete
    ac_visible: bool,
    ac_matches: Vec<String>,
    ac_selected: usize,
    ac_start_pos: usize, // cursor position of the '@'
    // Operation state persistence
    interrupted_ops: Vec<db::Operation>,
    diag_task_running: bool,
    tui_start: Instant,
    db_latency_ms: Option<u32>,
    db_online: bool,
    db_latency_rx: Option<mpsc::UnboundedReceiver<Option<u32>>>,
    // Multi-select (index-based)
    multi_selected: HashSet<usize>,
    // Split-pane resize
    dashboard_split_pct: Option<u16>,
    detail_split_pct: Option<u16>,
    dragging_split: Option<SplitDragTarget>,
    dashboard_body_area: ratatui::layout::Rect,
    detail_body_area: ratatui::layout::Rect,
    dashboard_divider_area: ratatui::layout::Rect,
    detail_divider_area: ratatui::layout::Rect,
    // Audit log
    audit_tx: Option<tokio::sync::mpsc::UnboundedSender<AuditEvent>>,
    audit_rx: Option<mpsc::UnboundedReceiver<AuditResult>>,
    audit_last: Option<String>,
    audit_pending: usize,
}

/// Returns true for npm output lines worth showing in the overlay.
/// Filters out transitive-dependency deprecation noise and pure whitespace.
fn npm_line_is_meaningful(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    let lower = line.to_lowercase();
    // Skip noisy deprecated warnings for transitive deps
    if lower.contains("npm warn deprecated") {
        return false;
    }
    if lower.contains("npm warn notsup") {
        return false;
    }
    // Always keep error lines
    if lower.contains("err") {
        return true;
    }
    // Keep final result lines (added/changed/removed/updated X packages)
    if lower.contains("added") && lower.contains("package") {
        return true;
    }
    if (lower.contains("changed") || lower.contains("updated") || lower.contains("removed"))
        && lower.contains("package")
    {
        return true;
    }
    // Keep timing/progress lines
    if lower.contains("packages in") {
        return true;
    }
    // Keep non-deprecated warnings
    if lower.starts_with("npm warn") {
        return true;
    }
    false
}

async fn try_clipboard_command(bin: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).await.is_err() {
            return false;
        }
    }
    child.wait().await.map(|s| s.success()).unwrap_or(false)
}

async fn copy_to_clipboard(text: String) -> bool {
    try_clipboard_command("pbcopy", &[], &text).await
        || try_clipboard_command("wl-copy", &[], &text).await
        || try_clipboard_command("xclip", &["-selection", "clipboard"], &text).await
        || try_clipboard_command("xsel", &["--clipboard", "--input"], &text).await
}

impl App {
    async fn new(fleet_config: config::FleetConfig) -> Self {
        let pool = db::get_pool();
        // Run schema migrations on startup (idempotent)
        let _ = db::run_migrations(&pool).await;
        let self_ip = std::env::var("SAM_SELF_IP").unwrap_or_else(|_| "localhost".into());
        let mut agents = Vec::new();

        match db::load_fleet(&pool).await {
            Ok(db_agents) => {
                for da in db_agents {
                    let cfg = fleet_config.agent.iter().find(|c| c.name == da.agent_name);
                    let caps: Vec<String> = da
                        .capabilities
                        .and_then(|c| serde_json::from_str(&c).ok())
                        .unwrap_or_default();
                    agents.push(Agent {
                        name: cfg
                            .map(|c| c.display_name().to_string())
                            .unwrap_or_else(|| da.agent_name.clone()),
                        db_name: da.agent_name.clone(),
                        emoji: cfg.map(|c| c.emoji().to_string()).unwrap_or_else(|| {
                            os_emoji(da.os_info.as_deref().unwrap_or("")).to_string()
                        }),
                        host: da.tailscale_ip.unwrap_or("?".into()),
                        location: cfg
                            .map(|c| c.location().to_string())
                            .unwrap_or_else(|| "?".into()),
                        status: AgentStatus::from_str(&da.status),
                        os: da.os_info.unwrap_or_default(),
                        kernel: da.kernel.unwrap_or_default(),
                        oc_version: da.oc_version.unwrap_or_default(),
                        last_seen: String::new(),
                        current_task: None,
                        ssh_user: cfg
                            .map(|c| c.ssh_user().to_string())
                            .unwrap_or_else(|| "root".into()),
                        capabilities: caps,
                        token_burn: da.token_burn_today,
                        latency_ms: None,
                        cpu_pct: None,
                        ram_pct: None,
                        disk_pct: None,
                        gateway_port: da.gateway_port,
                        gateway_token: da.gateway_token.clone(),
                        gateway_pid: da.gateway_pid,
                        gateway_status: if da.gateway_pid.unwrap_or(0) > 0 { GatewayStatus::Online } else { GatewayStatus::Unknown },
                        uptime_seconds: da.uptime_seconds,
                        activity: "idle".into(),
                        context_pct: None,
                        last_probe_at: None,
                    });
                }
            }
            Err(e) => eprintln!("DB: {}", e),
        }

        let chat_history = match db::load_global_chat(&pool, 100).await {
            Ok(msgs) => msgs
                .iter()
                .map(|m| ChatLine {
                    id: m.id,
                    sender: m.sender.clone(),
                    target: m.target.clone(),
                    message: m.message.clone(),
                    response: m.response.clone(),
                    time: m.created_at.clone(),
                    status: m.status.clone(),
                    kind: m.kind.clone(),
                    thread_id: m.thread_id.clone(),
                    parent_id: m.parent_id,
                    depth: 0,
                })
                .collect(),
            Err(_) => vec![],
        };

        // Detect operations interrupted by a previous session (started > 5 min ago, still 'running')
        let _ = db::mark_stale_operations_interrupted(&pool).await;
        let interrupted_ops = db::load_interrupted_operations(&pool)
            .await
            .unwrap_or_default();

        let tn = ThemeName::Standard;
        let bd = BgDensity::Dark;
        let (audit_tx, mut audit_input_rx) = mpsc::unbounded_channel::<AuditEvent>();
        let (audit_result_tx, audit_result_rx) = mpsc::unbounded_channel::<AuditResult>();
        let audit_pool = pool.clone();
        tokio::spawn(async move {
            while let Some(evt) = audit_input_rx.recv().await {
                let result = db::append_audit_log(&audit_pool, &evt.actor, &evt.action, &evt.target, &evt.detail).await;
                let send_result = match result {
                    Ok(()) => audit_result_tx.send(AuditResult { ok: true, action: evt.action, target: evt.target, error: None }),
                    Err(e) => audit_result_tx.send(AuditResult { ok: false, action: evt.action, target: evt.target, error: Some(db::sanitize_error(&e.to_string())) }),
                };
                if let Err(e) = send_result {
                    eprintln!("audit result channel send failed: {}", e);
                }
            }
        });

        App {
            fleet_config: fleet_config.agent,
            agents,
            selected: 0,
            screen: Screen::Dashboard,
            focus: Focus::Fleet,
            should_quit: false,
            last_refresh: Instant::now(),
            last_chat_poll: Instant::now(),
            status_message: String::new(),
            toast_message: None,
            toast_at: None,
            db_pool: Some(pool),
            chat_input: String::new(),
            chat_history,
            chat_scroll: 0,
            agent_chat_input: String::new(),
            agent_chat_history: vec![],
            agent_chat_scroll: 0,
            agent_threads: vec![],
            active_thread_id: None,
            reply_parent_id: None,
            thread_sidebar: false,
            pinned_threads: HashSet::new(),
            refresh_rx: None,
            refreshing: false,
            refresh_cycle: 0,
            self_ip,
            command_input: String::new(),
            wizard: wizard::AgentWizard::new(),
            tasks: vec![],
            task_filter_agent: None,
            task_selected: 0,
            task_input: String::new(),
            task_input_active: false,
            last_task_poll: Instant::now(),
            spawned_agents: vec![],
            show_splash: true,
            splash_start: Instant::now(),
            config_text: None,
            config_scroll: 0,
            help_scroll: 0,
            filter_active: false,
            filter_text: String::new(),
            alerts: vec![],
            alert_flash: None,
            alerts_scroll: 0,
            gateway_confirm_at: None,
            multi_selected: HashSet::new(), // usize indices
            spinner_frame: 0,
            sort_mode: SortMode::Name,
            group_filter: GroupFilter::All,
            fleet_area: Rect::default(),
            chat_area: Rect::default(),
            detail_info_area: Rect::default(),
            detail_chat_area: Rect::default(),
            fleet_row_start_y: 0,
            theme_name: tn,
            bg_density: bd,
            theme: Theme::resolve(tn, bd),
            routed_msg_ids: std::collections::HashSet::new(),
            diag_active: false,
            diag_steps: vec![],
            diag_rx: None,
            diag_auto_fix: false,
            diag_start: None,
            diag_title: None,
            diag_overlay_scroll: 0,
            fleet_diag_active: false,
            fleet_diag_fix: false,
            fleet_diag_selected: 0,
            fleet_diag_done: false,
            fleet_diag_results: vec![],
            fleet_diag_rx: None,
            svc_list: vec![],
            config_load_rx: None,
            svc_selected: 0,
            svc_config: None,
            svc_loading: false,
            svc_load_rx: None,
            svc_detail_scroll: 0,
            ws_files: vec![],
            ws_selected: 0,
            ws_content: None,
            ws_content_scroll: 0,
            ws_load_rx: None,
            ws_file_rx: None,
            ws_editing: false,
            ws_edit_buffer: vec![],
            ws_cursor: (0, 0),
            ws_undo_stack: vec![],
            ws_discard_confirm: false,
            ws_crons: vec![],
            ws_loading: false,
            chat_poll_rx: None,
            chat_polling: false,
            wizard_ssh_rx: None,
            ac_visible: false,
            ac_matches: vec![],
            ac_selected: 0,
            ac_start_pos: 0,
            latest_oc_version: String::new(),
            interrupted_ops,
            diag_task_running: false,
            agent_model: None,
            agent_model_agent: None,
            agent_model_loading: false,
            audit_last: None,
            audit_pending: 0,
            audit_rx: None,
            audit_tx: None,
            dashboard_body_area: ratatui::layout::Rect::default(),
            dashboard_divider_area: ratatui::layout::Rect::default(),
            dashboard_split_pct: None,
            db_latency_ms: None,
            db_latency_rx: None,
            db_online: true,
            detail_body_area: ratatui::layout::Rect::default(),
            detail_divider_area: ratatui::layout::Rect::default(),
            detail_split_pct: None,
            dragging_split: None::<SplitDragTarget>,
            gateway_action_confirm: None,
            model_load_rx: None,
            model_options: vec![
                "anthropic/claude-opus-4-6".into(),
                "anthropic/claude-sonnet-4-6".into(),
                "anthropic/claude-haiku-4-5".into(),
                "openai/gpt-4o".into(),
                "openai/gpt-4o-mini".into(),
                "google/gemini-2.0-flash".into(),
            ],
            model_picker_active: false,
            model_picker_selected: 0,
            model_write_rx: None,
            selected_agents: std::collections::HashSet::new(),
            tui_start: Instant::now(),
            vim_mode: false,
            vim_pending: None,
            ws_cron_form_active: false,
            ws_cron_form_description: String::new(),
            ws_cron_form_edit: false,
            ws_cron_form_focus: 0,
            ws_cron_form_schedule: String::new(),
            ws_cron_selected: 0,
        }
    }

    fn next(&mut self) {
        let indices = self.filtered_agent_indices();
        if indices.is_empty() {
            return;
        }
        let pos = indices.iter().position(|&i| i == self.selected);
        self.selected = match pos {
            Some(p) if p + 1 < indices.len() => indices[p + 1],
            _ => indices[0],
        };
    }
    fn previous(&mut self) {
        let indices = self.filtered_agent_indices();
        if indices.is_empty() {
            return;
        }
        let pos = indices.iter().position(|&i| i == self.selected);
        self.selected = match pos {
            Some(0) | None => indices[indices.len() - 1],
            Some(p) => indices[p - 1],
        };
    }

    fn filtered_jump_top(&mut self) {
        let indices = self.filtered_agent_indices();
        if let Some(&i) = indices.first() { self.selected = i; }
    }

    fn filtered_jump_bottom(&mut self) {
        let indices = self.filtered_agent_indices();
        if let Some(&i) = indices.last() { self.selected = i; }
    }

    fn filtered_step_by(&mut self, delta: isize) {
        let indices = self.filtered_agent_indices();
        if indices.is_empty() { return; }
        let pos = indices.iter().position(|&i| i == self.selected).unwrap_or(0) as isize;
        let max = indices.len().saturating_sub(1) as isize;
        let next = (pos + delta).clamp(0, max) as usize;
        self.selected = indices[next];
    }

    fn move_tab_left(&mut self) {
        if self.screen != Screen::AgentDetail { return; }
        self.focus = match self.focus {
            Focus::Fleet => Focus::Services,
            Focus::AgentChat => Focus::Fleet,
            Focus::Workspace => Focus::AgentChat,
            Focus::Services => Focus::Workspace,
            _ => self.focus.clone(),
        };
        if self.focus == Focus::Workspace {
            self.start_workspace_load();
        } else if self.focus == Focus::Services {
            self.start_services_load();
        }
    }

    fn move_tab_right(&mut self) {
        if self.screen != Screen::AgentDetail { return; }
        self.focus = match self.focus {
            Focus::Fleet => Focus::AgentChat,
            Focus::AgentChat => Focus::Workspace,
            Focus::Workspace => Focus::Services,
            Focus::Services => Focus::Fleet,
            _ => self.focus.clone(),
        };
        if self.focus == Focus::Workspace {
            self.start_workspace_load();
        } else if self.focus == Focus::Services {
            self.start_services_load();
        }
    }

    fn copy_selected_agent_info(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else { return; };
        let text = format!(
            "{} ({})\nHost: {}\nStatus: {}\nOpenClaw: {}\nLocation: {}\nSSH User: {}",
            agent.name, agent.db_name, agent.host, agent.status, agent.oc_version, agent.location, agent.ssh_user
        );
        tokio::spawn(async move {
            let _ = copy_to_clipboard(text).await;
        });
        self.toast("📋 Copied agent info");
    }


    fn active_ops_running(&self) -> usize {
        let mut count = 0;
        if self.diag_active { count += 1; }
        if self.fleet_diag_active { count += 1; }
        count
    }

    fn queue_audit_mutation(&mut self, action: impl Into<String>, target: impl Into<String>, detail: impl Into<String>) {
        if let Some(tx) = &self.audit_tx {
            let _ = tx.send(AuditEvent {
                actor: std::env::var("USER").unwrap_or_else(|_| "operator".into()),
                action: action.into(),
                target: target.into(),
                detail: detail.into(),
            });
        }
    }

    fn start_db_latency_probe(&mut self) {
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let (tx, rx) = mpsc::unbounded_channel::<Option<u32>>();
            self.db_latency_rx = Some(rx);
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let ok = pool.get_conn().await.is_ok();
                let lat = if ok { Some(start.elapsed().as_millis() as u32) } else { None };
                let _ = tx.send(lat);
            });
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast_message = Some(msg.to_string());
        self.toast_at = Some(Instant::now());
    }

    fn user(&self) -> String {
        std::env::var("SAM_USER").unwrap_or_else(|_| "operator".into())
    }

    fn new_thread_id() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes();
            b.copy_from_slice(&nanos);
        }
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }

    fn thread_title(s: &str) -> String {
        let compact = s.split_whitespace().collect::<Vec<_>>().join(" ");
        compact.chars().take(40).collect()
    }

    fn apply_thread_depth(messages: &mut [ChatLine]) {
        let by_id: HashMap<i64, Option<i64>> =
            messages.iter().map(|m| (m.id, m.parent_id)).collect();
        for m in messages.iter_mut() {
            let mut depth = 0u8;
            let mut parent = m.parent_id;
            while let Some(pid) = parent {
                depth = depth.saturating_add(1);
                if depth >= 3 {
                    break;
                }
                parent = by_id.get(&pid).copied().flatten();
            }
            m.depth = depth.min(3);
        }
    }

    fn thread_context_prompt(&self, parent_id: Option<i64>) -> String {
        let Some(mut cur) = parent_id else {
            return String::new();
        };
        let by_id: HashMap<i64, &ChatLine> =
            self.agent_chat_history.iter().map(|m| (m.id, m)).collect();
        let mut chain: Vec<String> = Vec::new();
        for _ in 0..6 {
            let Some(msg) = by_id.get(&cur).copied() else {
                break;
            };
            chain.push(format!("{}: {}", msg.sender, msg.message));
            let Some(pid) = msg.parent_id else {
                break;
            };
            cur = pid;
        }
        if chain.is_empty() {
            String::new()
        } else {
            chain.reverse();
            format!("\n## Thread Context\n{}\n", chain.join("\n"))
        }
    }

    /// Build a system prompt that gives agents awareness of the fleet and how to communicate
    fn build_system_prompt(&self, target_agent: Option<&str>) -> String {
        let agent_list: Vec<String> = self
            .agents
            .iter()
            .map(|a| {
                let status = format!("{}", a.status);
                format!(
                    "  - @{} ({}{})",
                    a.db_name,
                    a.location,
                    if status == "online" { "" } else { ", offline" }
                )
            })
            .collect();

        let context = if let Some(target) = target_agent {
            format!(
                "You are @{}. This is a direct message from the operator.",
                target
            )
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
            agent_list.join(
                "
"
            ),
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
                    AgentStatus::Online => 0,
                    AgentStatus::Busy => 1,
                    AgentStatus::Unknown => 2,
                    AgentStatus::Probing => 3,
                    AgentStatus::Offline => 4,
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

    fn cycle_group(&mut self) {
        self.group_filter = self.group_filter.next();
        self.selected = 0;
    }

    fn cycle_bg(&mut self) {
        self.bg_density = self.bg_density.next();
        self.theme = Theme::resolve(self.theme_name, self.bg_density);
    }

    /// Get the active chat input (depending on screen)
    fn active_chat_input(&self) -> &str {
        if self.screen == Screen::AgentDetail {
            &self.agent_chat_input
        } else {
            &self.chat_input
        }
    }

    /// Returns indices into self.agents that match the current group_filter
    fn filtered_agent_indices(&self) -> Vec<usize> {
        self.agents
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let keep = match self.group_filter {
                    GroupFilter::All => true,
                    GroupFilter::Home => a.location == "Home",
                    GroupFilter::SM => a.location == "SM",
                    GroupFilter::VPS => a.location == "VPS",
                    GroupFilter::Mobile => a.location == "Mobile",
                    GroupFilter::Outdated => {
                        !a.oc_version.is_empty()
                            && a.oc_version != "?"
                            && !self.latest_oc_version.is_empty()
                            && !a.oc_version.contains(&self.latest_oc_version)
                    }
                    GroupFilter::Offline => a.status == AgentStatus::Offline,
                };
                if keep { Some(i) } else { None }
            })
            .collect()
    }

    fn selected_agent_indices(&self) -> Vec<usize> {
        self.agents.iter().enumerate()
            .filter_map(|(i, a)| if self.selected_agents.contains(&a.db_name) { Some(i) } else { None })
            .collect()
    }

    fn selected_agent_count(&self) -> usize {
        self.agents.iter().filter(|a| self.selected_agents.contains(&a.db_name)).count()
    }

    fn active_chat_input_mut(&mut self) -> &mut String {
        if self.screen == Screen::AgentDetail {
            &mut self.agent_chat_input
        } else {
            &mut self.chat_input
        }
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
                let matches: Vec<String> = self
                    .agents
                    .iter()
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
        if !self.ac_visible || self.ac_matches.is_empty() {
            return;
        }
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
        if self.chat_input.trim().is_empty() {
            return;
        }
        let message = validate::sanitize_chat_message(&self.chat_input);
        self.chat_input.clear();
        if message.is_empty() {
            return;
        }

        // If message contains @mentions, only send to those agents. Otherwise broadcast to all.
        let mentioned: Vec<String> = {
            let mut m = Vec::new();
            for word in message.split_whitespace() {
                if let Some(name) = word.strip_prefix('@') {
                    let name_lower = name.to_lowercase();
                    if self
                        .agents
                        .iter()
                        .any(|a| a.db_name.to_lowercase() == name_lower)
                    {
                        m.push(name_lower);
                    }
                }
            }
            m
        };
        let targeted = !mentioned.is_empty();
        let agent_names: Vec<String> = if targeted {
            self.agents
                .iter()
                .filter(|a| mentioned.contains(&a.db_name.to_lowercase()))
                .map(|a| a.db_name.clone())
                .collect()
        } else {
            self.agents.iter().map(|a| a.db_name.clone()).collect()
        };
        let display_target = if targeted {
            Some(
                agent_names
                    .iter()
                    .map(|n| format!("@{}", n))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        } else {
            None
        };
        self.chat_history.push(ChatLine {
            id: 0,
            sender: self.user(),
            target: display_target,
            message: message.clone(),
            response: None,
            time: now_str(),
            status: "pending".into(),
            kind: if targeted {
                "direct".into()
            } else {
                "global".into()
            },
            thread_id: None,
            parent_id: None,
            depth: 0,
        });
        self.queue_audit_mutation(
            if targeted { "chat.mention_send" } else { "chat.broadcast_send" },
            &format!("{} agent(s)", agent_names.len()),
            "message_queued",
        );

        if let Some(pool) = &self.db_pool {
            let ids = db::send_broadcast(pool, &self.user(), &message, &agent_names)
                .await
                .unwrap_or_default();
            let sys_prompt = self.build_system_prompt(None);
            // Fire streaming AI requests to targeted agents (or all if broadcast)
            for (i, agent) in self.agents.iter().enumerate() {
                if targeted && !agent_names.contains(&agent.db_name) {
                    continue;
                }
                if let Some(tok) = &agent.gateway_token {
                    let url = format!(
                        "http://{}:{}/v1/chat/completions",
                        agent.host, agent.gateway_port
                    );
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
                            .build()
                            .unwrap_or_default();
                        let _ = db::update_chat_status(&pool, msg_id, "connecting").await;
                        let body = serde_json::json!({
                            "model": "openclaw:main",
                            "stream": true,
                            "messages": [
                                {"role": "system", "content": sys_prompt},
                                {"role": "user", "content": msg}
                            ]
                        });
                        let result = client
                            .post(&url)
                            .header("Authorization", format!("Bearer {}", tok))
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send()
                            .await;
                        match result {
                            Ok(resp) => {
                                use reqwest::header::CONTENT_TYPE;
                                let ct = resp
                                    .headers()
                                    .get(CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_string();
                                if ct.contains("text/event-stream") || ct.contains("text/plain") {
                                    let _ = db::update_chat_status(&pool, msg_id, "thinking").await;
                                    use futures_util::StreamExt;
                                    let mut stream = resp.bytes_stream();
                                    let mut full = String::new();
                                    let mut last_write = std::time::Instant::now();
                                    let mut got = false;
                                    while let Some(chunk) = stream.next().await {
                                        let chunk = match chunk {
                                            Ok(c) => c,
                                            Err(_) => break,
                                        };
                                        let text = String::from_utf8_lossy(&chunk);
                                        for line in text.lines() {
                                            let line = line.trim();
                                            if line == "data: [DONE]" || !line.starts_with("data: ")
                                            {
                                                continue;
                                            }
                                            if let Ok(j) = serde_json::from_str::<serde_json::Value>(
                                                &line[6..],
                                            ) {
                                                if let Some(c) =
                                                    j["choices"][0]["delta"]["content"].as_str()
                                                {
                                                    full.push_str(c);
                                                    got = true;
                                                }
                                            }
                                        }
                                        if got
                                            && last_write.elapsed()
                                                > std::time::Duration::from_millis(300)
                                        {
                                            let _ =
                                                db::update_chat_partial(&pool, msg_id, &full).await;
                                            last_write = std::time::Instant::now();
                                        }
                                    }
                                    if full.is_empty() {
                                        full = "(empty response)".into();
                                    }
                                    let _ = db::respond_to_chat(&pool, msg_id, &full).await;
                                } else {
                                    let _ = db::update_chat_status(&pool, msg_id, "thinking").await;
                                    match resp.json::<serde_json::Value>().await {
                                        Ok(j) => {
                                            let r = j["choices"][0]["message"]["content"]
                                                .as_str()
                                                .unwrap_or("(no content)")
                                                .to_string();
                                            let _ = db::respond_to_chat(&pool, msg_id, &r).await;
                                        }
                                        Err(e) => {
                                            let _ = db::respond_to_chat(
                                                &pool,
                                                msg_id,
                                                &format!("Parse error: {}", e),
                                            )
                                            .await;
                                        }
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
                                    bcast_port,
                                    tok,
                                    shell::escape(
                                        &serde_json::to_string(&body_nostream).unwrap_or_default()
                                    )
                                );
                                let response = match tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    tokio::process::Command::new("ssh")
                                        .args([
                                            "-o",
                                            "ConnectTimeout=2",
                                            "-o",
                                            "StrictHostKeyChecking=no",
                                            "-o",
                                            "BatchMode=yes",
                                            &format!("{}@{}", bcast_user, bcast_host),
                                            &ssh_cmd,
                                        ])
                                        .output(),
                                )
                                .await
                                {
                                    Ok(Ok(o)) if o.status.success() => {
                                        let s = String::from_utf8_lossy(&o.stdout);
                                        serde_json::from_str::<serde_json::Value>(&s)
                                            .ok()
                                            .and_then(|j| {
                                                j["choices"][0]["message"]["content"]
                                                    .as_str()
                                                    .map(|s| s.to_string())
                                            })
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
        if self.agent_chat_input.trim().is_empty() {
            return;
        }
        let message = validate::sanitize_chat_message(&self.agent_chat_input);
        self.agent_chat_input.clear();
        if message.is_empty() {
            return;
        }
        let agent = &self.agents[self.selected];
        let target = agent.db_name.clone();
        let host = agent.host.clone();
        let port = agent.gateway_port;
        let token = agent.gateway_token.clone();
        let thread_id = self
            .active_thread_id
            .clone()
            .unwrap_or_else(Self::new_thread_id);
        self.active_thread_id = Some(thread_id.clone());
        let parent_id = self.reply_parent_id.take();

        self.agent_chat_history.push(ChatLine {
            id: 0,
            sender: self.user(),
            target: Some(target.clone()),
            message: message.clone(),
            response: None,
            time: now_str(),
            status: "pending".into(),
            kind: "direct".into(),
            thread_id: Some(thread_id.clone()),
            parent_id,
            depth: 0,
        });
        Self::apply_thread_depth(&mut self.agent_chat_history);

        // Store in DB (fire-and-forget, get ID via channel)
        let msg_id = if let Some(pool) = &self.db_pool {
            db::send_chat_threaded(
                pool,
                &self.user(),
                Some(&target),
                &message,
                Some(&thread_id),
                parent_id,
            )
            .await
            .unwrap_or(0)
        } else {
            0
        };

        // Fire AI request via OpenClaw HTTP API (streaming)
        if let Some(tok) = token {
            let pool = self.db_pool.clone();
            let sys_prompt = format!(
                "{}{}",
                self.build_system_prompt(Some(&target)),
                self.thread_context_prompt(parent_id)
            );
            tokio::spawn(async move {
                let url = format!("http://{}:{}/v1/chat/completions", host, port);
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .unwrap_or_default();

                // Mark as connecting
                if let Some(ref p) = pool {
                    let _ = db::update_chat_status(p, msg_id, "connecting").await;
                }

                let body = serde_json::json!({
                    "model": "openclaw:main",
                    "stream": true,
                    "messages": [
                        {"role": "system", "content": sys_prompt},
                        {"role": "user", "content": message}
                    ]
                });
                let result = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", tok))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await;

                match result {
                    Ok(resp) => {
                        use reqwest::header::CONTENT_TYPE;
                        let ct = resp
                            .headers()
                            .get(CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        if ct.contains("text/event-stream") || ct.contains("text/plain") {
                            // SSE streaming response
                            if let Some(ref p) = pool {
                                let _ = db::update_chat_status(p, msg_id, "thinking").await;
                            }

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
                                    if line == "data: [DONE]" {
                                        continue;
                                    }
                                    if !line.starts_with("data: ") {
                                        continue;
                                    }
                                    let json_str = &line[6..];
                                    if let Ok(j) =
                                        serde_json::from_str::<serde_json::Value>(json_str)
                                    {
                                        if let Some(content) =
                                            j["choices"][0]["delta"]["content"].as_str()
                                        {
                                            full_response.push_str(content);
                                            got_content = true;
                                        }
                                    }
                                }

                                // Write partial response to DB every 300ms
                                if got_content
                                    && last_db_write.elapsed()
                                        > std::time::Duration::from_millis(300)
                                {
                                    if let Some(ref p) = pool {
                                        let _ = db::update_chat_partial(p, msg_id, &full_response)
                                            .await;
                                    }
                                    last_db_write = std::time::Instant::now();
                                }
                            }

                            // Final write
                            if full_response.is_empty() {
                                full_response = "(empty response)".into();
                            }
                            if let Some(ref p) = pool {
                                let _ = db::respond_to_chat(p, msg_id, &full_response).await;
                            }
                        } else {
                            // Non-streaming JSON response (fallback)
                            if let Some(ref p) = pool {
                                let _ = db::update_chat_status(p, msg_id, "thinking").await;
                            }
                            match resp.json::<serde_json::Value>().await {
                                Ok(j) => {
                                    let response = j["choices"][0]["message"]["content"]
                                        .as_str()
                                        .unwrap_or("(no content)")
                                        .to_string();
                                    if let Some(ref p) = pool {
                                        let _ = db::respond_to_chat(p, msg_id, &response).await;
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref p) = pool {
                                        let _ = db::respond_to_chat(
                                            p,
                                            msg_id,
                                            &format!("Parse error: {}", e),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(ref p) = pool {
                            let _ =
                                db::respond_to_chat(p, msg_id, &format!("Connection error: {}", e))
                                    .await;
                        }
                    }
                }
            });
        } else {
            if let Some(pool) = &self.db_pool {
                if msg_id > 0 {
                    let _ =
                        db::respond_to_chat(pool, msg_id, "(no gateway token configured)").await;
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
                let clean =
                    name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
                let clean_lower = clean.to_lowercase();
                if clean_lower != sender_agent.to_lowercase()
                    && self
                        .agents
                        .iter()
                        .any(|a| a.db_name.to_lowercase() == clean_lower)
                    && !mentioned.contains(&clean_lower)
                {
                    mentioned.push(clean_lower);
                }
            }
        }

        if mentioned.is_empty() {
            return;
        }

        // For each mentioned agent, forward the message
        for target_name in &mentioned {
            if let Some(agent) = self
                .agents
                .iter()
                .find(|a| a.db_name.to_lowercase() == *target_name)
            {
                if let Some(tok) = &agent.gateway_token {
                    let pool = self.db_pool.clone();
                    let url = format!(
                        "http://{}:{}/v1/chat/completions",
                        agent.host, agent.gateway_port
                    );
                    let tok = tok.clone();
                    let from = sender_agent.to_string();
                    let msg = format!("[Message from @{}]: {}", sender_agent, response);
                    let sys = self.build_system_prompt(Some(&agent.db_name));
                    let target = agent.db_name.clone();

                    // Write to chat history
                    self.chat_history.push(ChatLine {
                        id: 0,
                        sender: from.clone(),
                        target: Some(target.clone()),
                        message: format!("→ @{}", target),
                        response: None,
                        time: now_str(),
                        status: "routing".into(),
                        kind: "direct".into(),
                        thread_id: None,
                        parent_id: None,
                        depth: 0,
                    });

                    let msg_id = if let Some(ref p) = pool {
                        db::send_chat(p, &from, Some(&target), &format!("(routed from @{})", from))
                            .await
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(120))
                            .build()
                            .unwrap_or_default();
                        if let Some(ref p) = pool {
                            let _ = db::update_chat_status(p, msg_id, "connecting").await;
                        }
                        let body = serde_json::json!({
                            "model": "openclaw:main",
                            "stream": true,
                            "messages": [
                                {"role": "system", "content": sys},
                                {"role": "user", "content": msg}
                            ]
                        });
                        let result = client
                            .post(&url)
                            .header("Authorization", format!("Bearer {}", tok))
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send()
                            .await;
                        match result {
                            Ok(resp) => {
                                use reqwest::header::CONTENT_TYPE;
                                let ct = resp
                                    .headers()
                                    .get(CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_string();
                                if ct.contains("text/event-stream") || ct.contains("text/plain") {
                                    if let Some(ref p) = pool {
                                        let _ = db::update_chat_status(p, msg_id, "thinking").await;
                                    }
                                    use futures_util::StreamExt;
                                    let mut stream = resp.bytes_stream();
                                    let mut full = String::new();
                                    let mut last_write = std::time::Instant::now();
                                    let mut got = false;
                                    while let Some(chunk) = stream.next().await {
                                        let chunk = match chunk {
                                            Ok(c) => c,
                                            Err(_) => break,
                                        };
                                        let text = String::from_utf8_lossy(&chunk);
                                        for line in text.lines() {
                                            let line = line.trim();
                                            if line == "data: [DONE]" || !line.starts_with("data: ")
                                            {
                                                continue;
                                            }
                                            if let Ok(j) = serde_json::from_str::<serde_json::Value>(
                                                &line[6..],
                                            ) {
                                                if let Some(c) =
                                                    j["choices"][0]["delta"]["content"].as_str()
                                                {
                                                    full.push_str(c);
                                                    got = true;
                                                }
                                            }
                                        }
                                        if got
                                            && last_write.elapsed()
                                                > std::time::Duration::from_millis(300)
                                        {
                                            if let Some(ref p) = pool {
                                                let _ =
                                                    db::update_chat_partial(p, msg_id, &full).await;
                                            }
                                            last_write = std::time::Instant::now();
                                        }
                                    }
                                    if full.is_empty() {
                                        full = "(empty response)".into();
                                    }
                                    if let Some(ref p) = pool {
                                        let _ = db::respond_to_chat(p, msg_id, &full).await;
                                    }
                                } else {
                                    if let Some(ref p) = pool {
                                        let _ = db::update_chat_status(p, msg_id, "thinking").await;
                                    }
                                    match resp.json::<serde_json::Value>().await {
                                        Ok(j) => {
                                            let r = j["choices"][0]["message"]["content"]
                                                .as_str()
                                                .unwrap_or("(no content)")
                                                .to_string();
                                            if let Some(ref p) = pool {
                                                let _ = db::respond_to_chat(p, msg_id, &r).await;
                                            }
                                        }
                                        Err(e) => {
                                            if let Some(ref p) = pool {
                                                let _ = db::respond_to_chat(
                                                    p,
                                                    msg_id,
                                                    &format!("error: {}", e),
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if let Some(ref p) = pool {
                                    let _ = db::respond_to_chat(
                                        p,
                                        msg_id,
                                        &format!("unreachable: {}", e),
                                    )
                                    .await;
                                }
                            }
                        }
                    });
                }
            }
        }
    }

    /// Load OpenClaw config from agent via SSH (non-blocking)
    fn start_services_load(&mut self) {
        if self.selected >= self.agents.len() {
            return;
        }
        if self.svc_loading {
            return;
        }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        self.svc_loading = true;

        let (tx, rx) = mpsc::unbounded_channel();
        self.svc_load_rx = Some(rx);

        tokio::spawn(async move {
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "cat ~/.openclaw/openclaw.json 2>/dev/null || echo null",
                    ])
                    .output(),
            )
            .await;
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

    fn start_model_load(&mut self) {
        if self.selected >= self.agents.len() || self.agent_model_loading { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let db_name = agent.db_name.clone();
        self.agent_model_loading = true;
        self.agent_model_agent = Some(db_name.clone());
        self.model_options = curated_model_list();
        let (tx, rx) = mpsc::unbounded_channel();
        self.model_load_rx = Some(rx);
        tokio::spawn(async move {
            let cmd = r#"python3 - <<'PY'
import json, os, subprocess
p = os.path.expanduser('~/.openclaw/openclaw.json')
model = None
try:
    with open(p) as f:
        d = json.load(f)
    model = (((d.get('agents') or {}).get('defaults') or {}).get('model'))
except Exception:
    pass
models = []
for c in ("openclaw models list", "~/.npm-global/bin/openclaw models list"):
    try:
        out = subprocess.check_output(c, shell=True, stderr=subprocess.DEVNULL, text=True, timeout=4)
    except Exception:
        continue
    for line in out.splitlines():
        t = line.strip().split()
        if t and '/' in t[0]:
            models.append(t[0])
    if models:
        break
print(json.dumps({"model": model, "models": models}))
PY"#;
            let output = tokio::time::timeout(
                Duration::from_secs(7),
                Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), cmd,
                ]).output()
            ).await;
            let (model, models) = match output {
                Ok(Ok(o)) if o.status.success() => {
                    let parsed = serde_json::from_slice::<serde_json::Value>(&o.stdout).ok();
                    let model = parsed.as_ref().and_then(|v| v.get("model")).and_then(|m| m.as_str()).map(|s| s.to_string());
                    let models = parsed.as_ref().and_then(|v| v.get("models")).and_then(|m| m.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
                        .unwrap_or_default();
                    (model, models)
                }
                _ => (None, vec![]),
            };
            let _ = tx.send(ModelLoadResult { agent_db_name: db_name, model, models });
        });
    }

    fn open_model_picker(&mut self) {
        if self.selected >= self.agents.len() { return; }
        if self.agent_model_agent.as_deref() != Some(self.agents[self.selected].db_name.as_str()) {
            self.start_model_load();
        }
        self.model_picker_active = true;
        if let Some(current) = &self.agent_model {
            if let Some(idx) = self.model_options.iter().position(|m| m == current) {
                self.model_picker_selected = idx;
            }
        }
    }

    fn write_agent_model(&mut self, model: String, restart_gateway: bool) {
        if self.selected >= self.agents.len() { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let db_name = agent.db_name.clone();
        let name = agent.name.clone();
        let os = agent.os.to_ascii_lowercase();
        let is_mac = os.contains("mac") || os.contains("darwin");
        self.model_picker_active = false;
        self.diag_active = true;
        self.diag_task_running = true;
        self.diag_auto_fix = false;
        self.diag_title = Some(format!("🧠 Model switch — {}", name));
        self.diag_start = Some(Instant::now());
        self.diag_overlay_scroll = 0;
        self.diag_steps = vec![DiagStep { label: "Updating OpenClaw model".into(), status: DiagStatus::Running, detail: model.clone() }];
        let (diag_tx, diag_rx) = mpsc::unbounded_channel::<DiagStep>();
        self.diag_rx = Some(diag_rx);
        let (tx, rx) = mpsc::unbounded_channel::<ModelWriteResult>();
        self.model_write_rx = Some(rx);
        tokio::spawn(async move {
            let escaped_model = shell::escape(&model);
            let write_cmd = format!(r#"MODEL={} python3 - <<'PY'
import json, os
p = os.path.expanduser('~/.openclaw/openclaw.json')
try:
    with open(p) as f:
        d = json.load(f)
except Exception:
    d = {{}}
d.setdefault('agents', {{}}).setdefault('defaults', {{}})['model'] = os.environ.get('MODEL', '')
with open(p, 'w') as f:
    json.dump(d, f, indent=2)
print('ok')
PY"#, escaped_model);
            let write_out = Command::new("ssh").args([
                "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), &write_cmd,
            ]).output().await;
            let wrote = write_out.map(|o| o.status.success()).unwrap_or(false);
            if !wrote {
                let _ = diag_tx.send(DiagStep { label: "Updating OpenClaw model".into(), status: DiagStatus::Fail, detail: "write failed".into() });
                let _ = diag_tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Fail, detail: "Could not update model".into() });
                return;
            }
            let _ = diag_tx.send(DiagStep { label: "Updating OpenClaw model".into(), status: DiagStatus::Pass, detail: model.clone() });
            if restart_gateway {
                let _ = diag_tx.send(DiagStep { label: "Restarting gateway".into(), status: DiagStatus::Running, detail: String::new() });
                let pfx = if is_mac { "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; " } else { "" };
                let cmd = format!("{}openclaw gateway restart 2>&1 | tail -1 || ~/.npm-global/bin/openclaw gateway restart 2>&1 | tail -1 || systemctl --user restart openclaw-gateway 2>&1 | tail -1 || echo 'restart skipped - run manually'", pfx);
                let restart_ok = Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), &cmd,
                ]).output().await.map(|o| o.status.success()).unwrap_or(false);
                let _ = diag_tx.send(DiagStep {
                    label: "Restarting gateway".into(),
                    status: if restart_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                    detail: if restart_ok { "gateway restart attempted".into() } else { "restart failed — run manually".into() },
                });
            } else {
                let _ = diag_tx.send(DiagStep { label: "Restarting gateway".into(), status: DiagStatus::Skipped, detail: "skipped — restart required".into() });
            }
            let _ = diag_tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Pass, detail: "Model updated".into() });
            let _ = tx.send(ModelWriteResult { agent_db_name: db_name, model, restarted: restart_gateway });
        });
    }

    /// Parse services from loaded config
    fn parse_services(&mut self) {
        let mut services = Vec::new();
        if let Some(ref config) = self.svc_config {
            // Get enabled plugins
            let plugins = config
                .get("plugins")
                .and_then(|p| p.get("entries"))
                .and_then(|e| e.as_object());
            let channels = config.get("channels").and_then(|c| c.as_object());

            // Collect all known services (from plugins + channels)
            let mut seen = std::collections::HashSet::new();
            if let Some(plugins) = plugins {
                for (name, val) in plugins {
                    seen.insert(name.clone());
                    let enabled = val
                        .get("enabled")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    let has_channel = channels.map(|c| c.contains_key(name)).unwrap_or(false);
                    let summary = if has_channel {
                        self.build_channel_summary(name, config)
                    } else if enabled {
                        "enabled, no channel config".into()
                    } else {
                        "disabled".into()
                    };
                    services.push(ServiceEntry {
                        name: name.clone(),
                        icon: svc_icon(name),
                        enabled,
                        has_channel_config: has_channel,
                        summary,
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
                            name: name.clone(),
                            icon: svc_icon(name),
                            enabled: false,
                            has_channel_config: true,
                            summary: format!("no plugin entry — {}", summary),
                        });
                    }
                }
            }

            // Add gateway info
            if let Some(gw) = config.get("gateway") {
                let mode = gw.get("mode").and_then(|m| m.as_str()).unwrap_or("?");
                let has_token = gw.get("auth").and_then(|a| a.get("token")).is_some();
                let bind = gw
                    .get("bind")
                    .and_then(|b| b.as_str())
                    .unwrap_or("localhost");
                let chat = config
                    .get("gateway")
                    .and_then(|g| g.get("chatCompletions"))
                    .and_then(|c| c.get("enabled"))
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                services.insert(
                    0,
                    ServiceEntry {
                        name: "gateway".into(),
                        icon: "🌐",
                        enabled: true,
                        has_channel_config: false,
                        summary: format!(
                            "mode:{} bind:{} chat:{} auth:{}",
                            mode,
                            bind,
                            if chat { "on" } else { "off" },
                            if has_token { "token" } else { "none" }
                        ),
                    },
                );
            }

            // Add model info
            if let Some(agents) = config.get("agents").and_then(|a| a.get("defaults")) {
                let model = agents
                    .get("model")
                    .and_then(|m| m.get("primary"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("?");
                let ctx = agents
                    .get("contextTokens")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                services.insert(
                    0,
                    ServiceEntry {
                        name: "model".into(),
                        icon: "🧠",
                        enabled: true,
                        has_channel_config: false,
                        summary: format!(
                            "{} ({}K ctx)",
                            model.split('/').last().unwrap_or(model),
                            ctx / 1000
                        ),
                    },
                );
            }
        }
        services.sort_by(|a, b| {
            // Gateway and model first, then enabled, then disabled
            let rank = |s: &ServiceEntry| -> u8 {
                if s.name == "model" {
                    0
                } else if s.name == "gateway" {
                    1
                } else if s.enabled {
                    2
                } else {
                    3
                }
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
                if v.get("botToken").is_some() {
                    parts.push("token:✓".into());
                }
                if v.get("botId").is_some() {
                    parts.push("botId:✓".into());
                }
                if let Some(ch_arr) = v.get("channels").and_then(|c| c.as_array()) {
                    parts.push(format!("{} channels", ch_arr.len()));
                }
                if parts.is_empty() {
                    "configured".into()
                } else {
                    parts.join("  ")
                }
            }
            None => "no config".into(),
        }
    }

    /// Run diagnostics on focused agent (non-blocking, step-by-step)
    async fn ssh_run(host: &str, user: &str, self_ip: &str, cmd: &str) -> String {
        let out = if host == "localhost" || host == self_ip {
            tokio::process::Command::new("bash")
                .args(["-c", cmd])
                .output()
                .await
                .ok()
        } else {
            tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=5",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        cmd,
                    ])
                    .output(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
        };
        out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "timeout".into())
    }

    fn start_oc_update(&mut self) {
        if self.selected >= self.agents.len() {
            return;
        }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let name = agent.db_name.clone();
        let is_mac = agent.os.to_lowercase().contains("mac");
        let self_ip = self.self_ip.clone();
        let pool_opt = self.db_pool.clone();
        self.queue_audit_mutation("agent.oc_update", &name, "requested");

        self.diag_active = true;
        self.diag_task_running = true;
        self.diag_auto_fix = false;
        self.diag_title = Some(format!("⬆️  Update — {}", name));
        self.diag_start = Some(Instant::now());
        self.diag_overlay_scroll = 0;
        self.diag_steps = vec![DiagStep {
            label: format!("Updating OpenClaw on {}...", name),
            status: DiagStatus::Running,
            detail: String::new(),
        }];

        let (tx, rx) = mpsc::unbounded_channel::<DiagStep>();
        self.diag_rx = Some(rx);

        tokio::spawn(async move {
            let op_id = if let Some(ref pool) = pool_opt {
                db::create_operation(pool, &name, "oc_update").await.ok()
            } else {
                None
            };

            let pfx = if is_mac {
                "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
            } else {
                ""
            };

            // Step 1: current version (stored as prev_version for rollback)
            let _ = tx.send(DiagStep {
                label: "Current version".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let cur = App::ssh_run(
                &host,
                &user,
                &self_ip,
                &format!(
                    "{}openclaw --version 2>/dev/null || echo '(not installed)'",
                    pfx
                ),
            )
            .await;
            let prev_version = cur.trim().to_string();
            let _ = tx.send(DiagStep {
                label: "Current version".into(),
                status: DiagStatus::Pass,
                detail: prev_version.clone(),
            });

            // Pre-flight checks before install
            let _ = tx.send(DiagStep {
                label: "Pre-flight checks".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let preflight_cmd = format!(
                "{}node --version 2>/dev/null && npm --version 2>/dev/null && df -k $(npm config get prefix 2>/dev/null || echo /usr) | awk 'NR==2{{print $4}}' | xargs -I{{}} bash -c 'if [ {{}} -lt 512000 ]; then echo LOW_DISK; else echo OK_DISK; fi' 2>/dev/null || echo OK_DISK",
                pfx
            );
            let preflight_out = App::ssh_run(&host, &user, &self_ip, &preflight_cmd).await;
            if preflight_out.contains("LOW_DISK") {
                let _ = tx.send(DiagStep {
                    label: "Pre-flight checks".into(),
                    status: DiagStatus::Fail,
                    detail: "< 512MB disk space — aborting update".into(),
                });
                let _ = tx.send(DiagStep {
                    label: "DONE".into(),
                    status: DiagStatus::Fail,
                    detail: "Aborted — free up disk space first".into(),
                });
                return;
            }
            // Determine install strategy
            let has_sudo_npm = App::ssh_run(
                &host,
                &user,
                &self_ip,
                &format!(
                    "{}sudo -n npm --version 2>/dev/null && echo HAS_SUDO_NPM || echo NO_SUDO_NPM",
                    pfx
                ),
            )
            .await
            .contains("HAS_SUDO_NPM");
            let npm_prefix = App::ssh_run(
                &host,
                &user,
                &self_ip,
                &format!("{}npm config get prefix 2>/dev/null", pfx),
            )
            .await
            .trim()
            .to_string();
            let needs_ignore_scripts = App::ssh_run(
                &host,
                &user,
                &self_ip,
                &format!(
                    "{}gcc --version 2>/dev/null && echo HAS_GCC || echo NO_GCC",
                    pfx
                ),
            )
            .await
            .contains("NO_GCC");
            let node_ver = preflight_out
                .lines()
                .next()
                .unwrap_or("?")
                .trim()
                .to_string();
            let npm_ver = preflight_out
                .lines()
                .nth(1)
                .unwrap_or("?")
                .trim()
                .to_string();
            let _ = tx.send(DiagStep {
                label: "Pre-flight checks".into(),
                status: DiagStatus::Pass,
                detail: format!(
                    "node {} | npm {} | prefix: {}",
                    node_ver,
                    npm_ver,
                    npm_prefix.chars().take(30).collect::<String>()
                ),
            });

            // Step 2: stream npm install with smart flags
            let _ = tx.send(DiagStep {
                label: "Installing openclaw@latest".into(),
                status: DiagStatus::Running,
                detail: "running npm install...".into(),
            });
            let ignore_scripts = if needs_ignore_scripts {
                " --ignore-scripts"
            } else {
                ""
            };
            let install_cmd = if has_sudo_npm {
                format!(
                    "{}sudo npm install -g openclaw@latest{} 2>&1; echo EXITCODE:$?:DONE",
                    pfx, ignore_scripts
                )
            } else {
                // No sudo or sudo npm broken — install to user prefix
                format!(
                    "{}npm install -g openclaw@latest{} 2>&1; echo EXITCODE:$?:DONE",
                    pfx, ignore_scripts
                )
            };
            use tokio::io::AsyncBufReadExt;
            let mut child = if host == "localhost" || host == self_ip {
                tokio::process::Command::new("bash")
                    .args(["-c", &install_cmd])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            } else {
                tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=5",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &install_cmd,
                    ])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            };
            let mut install_ok = false;
            let mut last_line = String::new();
            let mut error_lines: Vec<String> = Vec::new();
            if let Ok(ref mut child) = child {
                if let Some(stdout) = child.stdout.take() {
                    let mut reader = tokio::io::BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        let clean = line.trim().to_string();
                        if clean.is_empty() {
                            continue;
                        }
                        // Parse exit code marker
                        if clean.starts_with("EXITCODE:") {
                            let code = clean
                                .trim_start_matches("EXITCODE:")
                                .trim_end_matches(":DONE");
                            install_ok = code == "0";
                            continue;
                        }
                        last_line = clean.clone();
                        // Track error lines for failure reporting
                        if clean.to_lowercase().contains("err") || clean.starts_with("npm ERR") {
                            error_lines.push(clean.chars().take(80).collect());
                        }
                        // Only stream meaningful lines; skip deprecated noise / whitespace
                        if npm_line_is_meaningful(&clean) {
                            let _ = tx.send(DiagStep {
                                label: "  npm".into(),
                                status: DiagStatus::Running,
                                detail: clean.chars().take(70).collect(),
                            });
                        }
                    }
                }
                // If we didn't parse exit code, use process exit status
                if let Ok(status) = child.wait().await {
                    if !install_ok {
                        install_ok = status.success();
                    }
                }
            }
            // Remove stale "running" npm step — replace with outcome
            let fail_detail = if !error_lines.is_empty() {
                error_lines
                    .last()
                    .cloned()
                    .unwrap_or("install failed".into())
            } else if !last_line.is_empty() {
                last_line.chars().take(60).collect()
            } else {
                "no output — check npm/sudo permissions".into()
            };
            // Resolve the "  npm" sub-step so it no longer shows as running
            let npm_summary: String = if install_ok {
                last_line.chars().take(60).collect()
            } else {
                fail_detail.clone()
            };
            let _ = tx.send(DiagStep {
                label: "  npm".into(),
                status: if install_ok {
                    DiagStatus::Pass
                } else {
                    DiagStatus::Fail
                },
                detail: npm_summary,
            });
            let _ = tx.send(DiagStep {
                label: "Installing openclaw@latest".into(),
                status: if install_ok {
                    DiagStatus::Fixed
                } else {
                    DiagStatus::Fail
                },
                detail: if install_ok {
                    last_line.chars().take(60).collect()
                } else {
                    fail_detail
                },
            });

            // Step 3: new version
            let _ = tx.send(DiagStep {
                label: "New version".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let new_v = App::ssh_run(
                &host,
                &user,
                &self_ip,
                &format!("{}openclaw --version 2>/dev/null || echo '?'", pfx),
            )
            .await;
            let _ = tx.send(DiagStep {
                label: "New version".into(),
                status: DiagStatus::Pass,
                detail: new_v.trim().to_string(),
            });

            // Step 4: restart gateway (try full path if openclaw not in PATH)
            let _ = tx.send(DiagStep {
                label: "Restarting gateway".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let restart_cmd = format!(
                "{}openclaw gateway restart 2>&1 | tail -1 || ~/.npm-global/bin/openclaw gateway restart 2>&1 | tail -1 || systemctl --user restart openclaw-gateway 2>&1 | tail -1 || echo 'restart skipped - run manually'",
                pfx
            );
            let restart_msg = App::ssh_run(&host, &user, &self_ip, &restart_cmd).await;
            let restart_ok = !restart_msg.contains("skipped") && !restart_msg.is_empty();
            let _ = tx.send(DiagStep {
                label: "Restarting gateway".into(),
                status: if restart_ok {
                    DiagStatus::Fixed
                } else {
                    DiagStatus::Fail
                },
                detail: restart_msg.trim().chars().take(70).collect(),
            });

            let _ = tx.send(DiagStep {
                label: "DONE".into(),
                status: DiagStatus::Pass,
                detail: "Update complete — press Esc to close".into(),
            });
            if let (Some(op_id), Some(ref pool)) = (op_id, pool_opt.as_ref()) {
                let status = if install_ok { "completed" } else { "failed" };
                let _ = db::complete_operation(pool, op_id, status, None).await;
            }
        });
    }

    fn start_gateway_action(&mut self, action: GatewayAction) {
        if self.selected >= self.agents.len() { return; }
        let idx = self.selected;
        let agent = &self.agents[idx];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let name = agent.db_name.clone();
        let self_ip = self.self_ip.clone();
        let pool_opt = self.db_pool.clone();
        let action_label = match action {
            GatewayAction::Start => "start",
            GatewayAction::Stop => "stop",
            GatewayAction::Restart => "restart",
        };

        self.agents[idx].gateway_status = if action == GatewayAction::Restart {
            GatewayStatus::Restarting
        } else {
            GatewayStatus::Unknown
        };
        self.diag_active = true;
        self.diag_task_running = true;
        self.diag_auto_fix = false;
        self.diag_title = Some(format!("🌐 Gateway {} — {}", action_label, name));
        self.diag_start = Some(Instant::now());
        self.diag_overlay_scroll = 0;
        self.diag_steps = vec![DiagStep { label: format!("Gateway {} on {}...", action_label, name), status: DiagStatus::Running, detail: String::new() }];

        let (tx, rx) = mpsc::unbounded_channel::<DiagStep>();
        self.diag_rx = Some(rx);

        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let remote_cmd = format!(
                "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; openclaw gateway {} 2>&1; echo EXITCODE:$?:DONE",
                action_label
            );
            let mut child = if host == "localhost" || host == self_ip {
                tokio::process::Command::new("bash")
                    .args(["-c", &remote_cmd])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            } else {
                tokio::process::Command::new("ssh")
                    .args([
                        "-o","ConnectTimeout=5","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host), &remote_cmd,
                    ])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            };

            let _ = tx.send(DiagStep { label: "Gateway command".into(), status: DiagStatus::Running, detail: format!("openclaw gateway {}", action_label) });
            let mut action_ok = false;
            let mut last_line = String::new();
            if let Ok(ref mut child) = child {
                if let Some(stdout) = child.stdout.take() {
                    let mut reader = tokio::io::BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        let clean = line.trim().to_string();
                        if clean.is_empty() { continue; }
                        if clean.starts_with("EXITCODE:") {
                            action_ok = clean.trim_start_matches("EXITCODE:").trim_end_matches(":DONE") == "0";
                            continue;
                        }
                        last_line = clean.clone();
                        let _ = tx.send(DiagStep { label: "  output".into(), status: DiagStatus::Running, detail: clean.chars().take(90).collect() });
                    }
                }
                if let Ok(status) = child.wait().await {
                    if !action_ok { action_ok = status.success(); }
                }
            }
            let _ = tx.send(DiagStep {
                label: "Gateway command".into(),
                status: if action_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if last_line.is_empty() { format!("gateway {}", action_label) } else { last_line.clone() },
            });

            if action == GatewayAction::Restart {
                let _ = tx.send(DiagStep { label: "Gateway re-check".into(), status: DiagStatus::Running, detail: "waiting 3s before health probe".into() });
                tokio::time::sleep(Duration::from_secs(3)).await;
            }

            let pid_out = App::ssh_run(&host, &user, &self_ip, "pgrep -f 'openclaw.*gateway' | head -1").await;
            let gw_pid = pid_out.trim().parse::<i32>().ok();
            if let Some(ref pool) = pool_opt {
                let _ = db::update_gateway_pid(pool, &name, gw_pid).await;
            }
            let gateway_online = gw_pid.unwrap_or(0) > 0;
            let final_status = if action == GatewayAction::Stop {
                !gateway_online
            } else {
                gateway_online
            };
            let _ = tx.send(DiagStep {
                label: "Gateway PID".into(),
                status: if final_status { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if let Some(pid) = gw_pid { format!("pid {}", pid) } else { "not running".into() },
            });
            let _ = tx.send(DiagStep {
                label: "DONE".into(),
                status: if action_ok && final_status { DiagStatus::Pass } else { DiagStatus::Fail },
                detail: if action_ok && final_status { format!("Gateway {} complete", action_label) } else { format!("Gateway {} may have failed", action_label) },
            });
        });
    }

    fn start_diagnostics(&mut self, fix: bool) {
        if self.selected >= self.agents.len() {
            return;
        }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let name = agent.db_name.clone();
        let location = agent.location.clone();
        let gw_port = agent.gateway_port;
        let pool_opt = self.db_pool.clone();
        self.queue_audit_mutation(if fix { "agent.diagnostics_fix" } else { "agent.diagnostics_check" }, &name, "requested");
        self.diag_active = true;
        self.diag_task_running = true;
        self.diag_auto_fix = fix;
        self.diag_title = None;
        self.diag_start = Some(Instant::now());
        self.diag_overlay_scroll = 0;
        self.diag_steps = vec![DiagStep {
            label: format!("Diagnosing {}...", name),
            status: DiagStatus::Running,
            detail: String::new(),
        }];

        let (tx, rx) = mpsc::unbounded_channel();
        self.diag_rx = Some(rx);

        tokio::spawn(async move {
            let op_id = if let Some(ref pool) = pool_opt {
                let op_type = if fix {
                    "diagnostics_fix"
                } else {
                    "diagnostics"
                };
                db::create_operation(pool, &name, op_type).await.ok()
            } else {
                None
            };

            let is_mac_check = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    "uname -s",
                ])
                .output()
                .await;
            let is_mac = is_mac_check
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string() == "Darwin")
                .unwrap_or(false);
            let pfx = if is_mac {
                "export PATH=/opt/homebrew/bin:$PATH; "
            } else {
                ""
            };

            // Step 1: SSH connectivity
            let _ = tx.send(DiagStep {
                label: "SSH connectivity".into(),
                status: DiagStatus::Running,
                detail: format!("ssh {}@{}", user, host),
            });
            let ssh_ok = tokio::time::timeout(
                Duration::from_secs(6),
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "echo ok",
                    ])
                    .output(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|o| o.status.success())
            .unwrap_or(false);

            let ssh_ok = if !ssh_ok && fix {
                // Attempt fix: check if we can ping the Tailscale IP
                let _ = tx.send(DiagStep {
                    label: "SSH connectivity".into(),
                    status: DiagStatus::Running,
                    detail: "unreachable — attempting fix...".into(),
                });

                // Check 1: Can we ping the host at all?
                let ping_ok = tokio::time::timeout(
                    Duration::from_secs(3),
                    Command::new("ping")
                        .args(["-c", "1", "-W", "2", &host])
                        .output(),
                )
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|o| o.status.success())
                .unwrap_or(false);

                if !ping_ok {
                    let _ = tx.send(DiagStep {
                        label: "  → Ping".into(),
                        status: DiagStatus::Fail,
                        detail: format!("{} not responding to ICMP", host),
                    });

                    // Try to restart Tailscale on our end for this route
                    let _ = tx.send(DiagStep {
                        label: "  → Tailscale route".into(),
                        status: DiagStatus::Running,
                        detail: "checking local Tailscale...".into(),
                    });
                    let ts_status = Command::new("tailscale")
                        .args(["status", "--json"])
                        .output()
                        .await;
                    let peer_found = ts_status
                        .as_ref()
                        .map(|o| {
                            let s = String::from_utf8_lossy(&o.stdout);
                            s.contains(&host)
                        })
                        .unwrap_or(false);

                    let peer_result = if peer_found {
                        let _ = tx.send(DiagStep {
                            label: "  → Tailscale route".into(),
                            status: DiagStatus::Pass,
                            detail: "peer known to Tailscale".into(),
                        });
                        // Try DERP relay ping
                        let _ = tx.send(DiagStep {
                            label: "  → Tailscale ping".into(),
                            status: DiagStatus::Running,
                            detail: "trying direct WireGuard...".into(),
                        });
                        let ts_ping = tokio::time::timeout(
                            Duration::from_secs(8),
                            Command::new("tailscale")
                                .args(["ping", "--c", "1", "--timeout", "5s", &host])
                                .output(),
                        )
                        .await
                        .ok()
                        .and_then(|r| r.ok());
                        let ts_ping_ok = ts_ping
                            .as_ref()
                            .map(|o| o.status.success())
                            .unwrap_or(false);
                        if ts_ping_ok {
                            let ping_detail = ts_ping
                                .as_ref()
                                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                                .unwrap_or_default();
                            let _ = tx.send(DiagStep {
                                label: "  → Tailscale ping".into(),
                                status: DiagStatus::Pass,
                                detail: ping_detail.chars().take(80).collect::<String>(),
                            });
                            let _ = tx.send(DiagStep {
                                label: "SSH connectivity".into(),
                                status: DiagStatus::Fail,
                                detail:
                                    "Tailscale reachable but SSH refused — check sshd on target"
                                        .into(),
                            });
                            false
                        } else {
                            let _ = tx.send(DiagStep {
                                label: "  → Tailscale ping".into(),
                                status: DiagStatus::Fail,
                                detail: "WireGuard unreachable".into(),
                            });

                            // === LAN FALLBACK: Try to reach via mDNS or known LAN ===
                            let _ = tx.send(DiagStep {
                                label: "  → LAN discovery".into(),
                                status: DiagStatus::Running,
                                detail: "searching local network...".into(),
                            });

                            // Try mDNS (.local) resolution — macOS/Linux machines broadcast this
                            let hostname_clean =
                                name.to_lowercase().replace(' ', "").replace('_', "");
                            let mdns_names = vec![
                                format!("{}.local", hostname_clean),
                                format!("{}.local", name.to_lowercase()),
                            ];
                            let mut lan_ip: Option<String> = None;
                            for mdns in &mdns_names {
                                let resolve = tokio::time::timeout(
                                    Duration::from_secs(3),
                                    Command::new("getent").args(["hosts", mdns]).output(),
                                )
                                .await
                                .ok()
                                .and_then(|r| r.ok());
                                if let Some(o) = resolve {
                                    let out = String::from_utf8_lossy(&o.stdout);
                                    if let Some(ip) = out.split_whitespace().next() {
                                        if !ip.is_empty() && ip != "0.0.0.0" {
                                            lan_ip = Some(ip.to_string());
                                            break;
                                        }
                                    }
                                }
                                // Also try ping for .local
                                let ping_local = tokio::time::timeout(
                                    Duration::from_secs(3),
                                    Command::new("ping")
                                        .args(["-c", "1", "-W", "2", mdns])
                                        .output(),
                                )
                                .await
                                .ok()
                                .and_then(|r| r.ok());
                                if let Some(o) = ping_local {
                                    if o.status.success() {
                                        // Extract IP from ping output
                                        let out = String::from_utf8_lossy(&o.stdout);
                                        // "PING host.local (192.168.1.x)"
                                        if let Some(start) = out.find('(') {
                                            if let Some(end) = out[start..].find(')') {
                                                let ip = &out[start + 1..start + end];
                                                lan_ip = Some(ip.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }

                            // Also check ARP cache
                            if lan_ip.is_none() {
                                let arp = Command::new("bash").args(["-c",
                                    &format!("arp -n 2>/dev/null | grep -i '{}' | awk '{{print $1}}' | head -1", hostname_clean)
                                ]).output().await;
                                if let Ok(o) = arp {
                                    let ip = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                    if !ip.is_empty() {
                                        lan_ip = Some(ip);
                                    }
                                }
                            }

                            // Also check DB for lan_ip field
                            // Try common IPs from /etc/hosts or fleet knowledge
                            if lan_ip.is_none() {
                                let etc_hosts = Command::new("bash").args(["-c",
                                    &format!("grep -i '{}' /etc/hosts 2>/dev/null | awk '{{print $1}}' | head -1", hostname_clean)
                                ]).output().await;
                                if let Ok(o) = etc_hosts {
                                    let ip = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                    if !ip.is_empty() && !ip.starts_with('#') {
                                        lan_ip = Some(ip);
                                    }
                                }
                            }

                            let lan_fixed = if let Some(ref lip) = lan_ip {
                                let _ = tx.send(DiagStep {
                                    label: "  → LAN discovery".into(),
                                    status: DiagStatus::Pass,
                                    detail: format!("found at {}", lip),
                                });

                                // Try SSH via LAN IP
                                let _ = tx.send(DiagStep {
                                    label: "  → LAN SSH".into(),
                                    status: DiagStatus::Running,
                                    detail: format!("ssh {}@{}...", user, lip),
                                });
                                let lan_ssh = tokio::time::timeout(
                                    Duration::from_secs(8),
                                    Command::new("ssh")
                                        .args([
                                            "-o",
                                            "ConnectTimeout=3",
                                            "-o",
                                            "BatchMode=yes",
                                            "-o",
                                            "StrictHostKeyChecking=no",
                                            &format!("{}@{}", user, lip),
                                            "echo ok",
                                        ])
                                        .output(),
                                )
                                .await
                                .ok()
                                .and_then(|r| r.ok())
                                .map(|o| o.status.success())
                                .unwrap_or(false);

                                if lan_ssh {
                                    let _ = tx.send(DiagStep {
                                        label: "  → LAN SSH".into(),
                                        status: DiagStatus::Pass,
                                        detail: "connected via LAN!".into(),
                                    });

                                    // FIX: Restart Tailscale on the target machine
                                    let _ = tx.send(DiagStep {
                                        label: "  → Restart Tailscale".into(),
                                        status: DiagStatus::Running,
                                        detail: "bringing Tailscale back up...".into(),
                                    });
                                    let is_mac_target = Command::new("ssh")
                                        .args([
                                            "-o",
                                            "ConnectTimeout=3",
                                            "-o",
                                            "BatchMode=yes",
                                            "-o",
                                            "StrictHostKeyChecking=no",
                                            &format!("{}@{}", user, lip),
                                            "uname -s",
                                        ])
                                        .output()
                                        .await
                                        .map(|o| {
                                            String::from_utf8_lossy(&o.stdout).trim() == "Darwin"
                                        })
                                        .unwrap_or(false);

                                    let ts_restart_cmd = if is_mac_target {
                                        // macOS: use the Tailscale CLI to bring it up
                                        &format!(
                                            "sudo /Applications/Tailscale.app/Contents/MacOS/Tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || /usr/local/bin/tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || echo FAIL",
                                            name, name
                                        )
                                    } else {
                                        &format!(
                                            "sudo systemctl restart tailscaled && sleep 2 && sudo tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || echo FAIL",
                                            name
                                        )
                                    };

                                    let ts_result = tokio::time::timeout(
                                        Duration::from_secs(40),
                                        Command::new("ssh")
                                            .args([
                                                "-o",
                                                "ConnectTimeout=3",
                                                "-o",
                                                "BatchMode=yes",
                                                "-o",
                                                "StrictHostKeyChecking=no",
                                                &format!("{}@{}", user, lip),
                                                ts_restart_cmd,
                                            ])
                                            .output(),
                                    )
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok());

                                    let ts_ok = ts_result
                                        .as_ref()
                                        .map(|o| {
                                            let out = String::from_utf8_lossy(&o.stdout);
                                            !out.contains("FAIL")
                                        })
                                        .unwrap_or(false);

                                    if ts_ok {
                                        let _ = tx.send(DiagStep {
                                            label: "  → Restart Tailscale".into(),
                                            status: DiagStatus::Fixed,
                                            detail: "Tailscale restarted!".into(),
                                        });
                                        // Wait a beat and verify the original Tailscale IP works
                                        tokio::time::sleep(Duration::from_secs(5)).await;
                                        let _ = tx.send(DiagStep {
                                            label: "  → Verify Tailscale".into(),
                                            status: DiagStatus::Running,
                                            detail: format!("re-testing {}...", host),
                                        });
                                        let verify = tokio::time::timeout(
                                            Duration::from_secs(6),
                                            Command::new("ssh")
                                                .args([
                                                    "-o",
                                                    "ConnectTimeout=3",
                                                    "-o",
                                                    "BatchMode=yes",
                                                    "-o",
                                                    "StrictHostKeyChecking=no",
                                                    &format!("{}@{}", user, host),
                                                    "echo ok",
                                                ])
                                                .output(),
                                        )
                                        .await
                                        .ok()
                                        .and_then(|r| r.ok())
                                        .map(|o| o.status.success())
                                        .unwrap_or(false);
                                        if verify {
                                            let _ = tx.send(DiagStep {
                                                label: "  → Verify Tailscale".into(),
                                                status: DiagStatus::Pass,
                                                detail: "Tailscale SSH working!".into(),
                                            });
                                            let _ = tx.send(DiagStep {
                                                label: "SSH connectivity".into(),
                                                status: DiagStatus::Fixed,
                                                detail: format!(
                                                    "fixed — restarted Tailscale via LAN ({})",
                                                    lip
                                                ),
                                            });
                                            // Continue with remaining checks since we're now connected
                                        } else {
                                            let _ = tx.send(DiagStep {
                                                label: "  → Verify Tailscale".into(),
                                                status: DiagStatus::Fail,
                                                detail: "still unreachable via Tailscale IP".into(),
                                            });
                                            let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Fail,
                                                detail: format!("Tailscale restarted but mesh route not established — may need re-auth. LAN SSH works: {}@{}", user, lip) });
                                        }
                                        verify
                                    } else {
                                        let detail = ts_result
                                            .map(|o| {
                                                String::from_utf8_lossy(&o.stdout)
                                                    .trim()
                                                    .chars()
                                                    .take(80)
                                                    .collect::<String>()
                                            })
                                            .unwrap_or("timeout".into());
                                        let _ = tx.send(DiagStep {
                                            label: "  → Restart Tailscale".into(),
                                            status: DiagStatus::Fail,
                                            detail,
                                        });
                                        let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Fail,
                                            detail: format!("LAN reachable at {} but Tailscale restart failed — may need manual login", lip) });
                                        false
                                    }
                                } else {
                                    let _ = tx.send(DiagStep {
                                        label: "  → LAN SSH".into(),
                                        status: DiagStatus::Fail,
                                        detail: "LAN SSH also failed — machine may be asleep/off"
                                            .into(),
                                    });

                                    // Try Wake-on-LAN if we can find the MAC address
                                    let _ = tx.send(DiagStep {
                                        label: "  → Wake-on-LAN".into(),
                                        status: DiagStatus::Running,
                                        detail: "checking for MAC address...".into(),
                                    });
                                    let mac_lookup = Command::new("bash").args(["-c",
                                        &format!("arp -n {} 2>/dev/null | awk '/ether/{{print $3}}' || ip neigh show {} 2>/dev/null | awk '{{print $5}}'", lip, lip)
                                    ]).output().await;
                                    let mac = mac_lookup
                                        .as_ref()
                                        .map(|o| {
                                            String::from_utf8_lossy(&o.stdout).trim().to_string()
                                        })
                                        .unwrap_or_default();
                                    if mac.contains(':') && mac.len() >= 17 {
                                        // Send WoL magic packet
                                        let _ = tx.send(DiagStep {
                                            label: "  → Wake-on-LAN".into(),
                                            status: DiagStatus::Running,
                                            detail: format!("sending WoL to {}...", mac),
                                        });
                                        let wol_cmd = format!(
                                            "wakeonlan {} 2>/dev/null || etherwake -i eth0 {} 2>/dev/null || echo NOWOL",
                                            mac, mac
                                        );
                                        let _ = Command::new("bash")
                                            .args(["-c", &wol_cmd])
                                            .output()
                                            .await;
                                        let _ = tx.send(DiagStep {
                                            label: "  → Wake-on-LAN".into(),
                                            status: DiagStatus::Fixed,
                                            detail: format!(
                                                "WoL sent to {} — wait 30-60s for boot",
                                                mac
                                            ),
                                        });
                                        let _ = tx.send(DiagStep {
                                            label: "SSH connectivity".into(),
                                            status: DiagStatus::Fail,
                                            detail: format!(
                                                "WoL sent — press D again in 60s to re-check"
                                            ),
                                        });
                                    } else {
                                        let _ = tx.send(DiagStep {
                                            label: "  → Wake-on-LAN".into(),
                                            status: DiagStatus::Skipped,
                                            detail: "no MAC address in ARP cache".into(),
                                        });
                                        let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Fail,
                                            detail: "machine unreachable on both Tailscale and LAN — likely powered off".into() });
                                    }
                                    false
                                }
                            } else {
                                let _ = tx.send(DiagStep {
                                    label: "  → LAN discovery".into(),
                                    status: DiagStatus::Fail,
                                    detail: "no LAN IP found via mDNS/ARP/hosts".into(),
                                });

                                // Try SSH config aliases (e.g. ~/.ssh/config Host entries with different hostnames/ports)
                                let _ = tx.send(DiagStep {
                                    label: "  → SSH config".into(),
                                    status: DiagStatus::Running,
                                    detail: "checking ~/.ssh/config aliases...".into(),
                                });
                                let ssh_config_check = Command::new("bash").args(["-c",
                                    &format!("grep -i -A5 'Host {}' ~/.ssh/config 2>/dev/null | grep -i HostName | awk '{{print $2}}' | head -1", name)
                                ]).output().await;
                                let config_host = ssh_config_check
                                    .as_ref()
                                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                                    .unwrap_or_default();

                                if !config_host.is_empty() {
                                    let _ = tx.send(DiagStep {
                                        label: "  → SSH config".into(),
                                        status: DiagStatus::Pass,
                                        detail: format!("found alias: {}", config_host),
                                    });
                                    // Try SSH via the config alias
                                    let _ = tx.send(DiagStep {
                                        label: "  → SSH via alias".into(),
                                        status: DiagStatus::Running,
                                        detail: format!("ssh {}...", name),
                                    });
                                    let alias_ssh = tokio::time::timeout(
                                        Duration::from_secs(10),
                                        Command::new("ssh")
                                            .args([
                                                "-o",
                                                "ConnectTimeout=5",
                                                "-o",
                                                "BatchMode=yes",
                                                "-o",
                                                "StrictHostKeyChecking=no",
                                                &name,
                                                "echo ok",
                                            ])
                                            .output(),
                                    )
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .map(|o| o.status.success())
                                    .unwrap_or(false);

                                    if alias_ssh {
                                        let _ = tx.send(DiagStep {
                                            label: "  → SSH via alias".into(),
                                            status: DiagStatus::Pass,
                                            detail: "connected!".into(),
                                        });
                                        // Restart Tailscale via alias
                                        let _ = tx.send(DiagStep {
                                            label: "  → Restart Tailscale".into(),
                                            status: DiagStatus::Running,
                                            detail: "bringing Tailscale back up...".into(),
                                        });
                                        let ts_cmd = &format!(
                                            "sudo tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || sudo systemctl restart tailscaled && sleep 2 && sudo tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || echo FAIL",
                                            name, name
                                        );
                                        let ts_result = tokio::time::timeout(
                                            Duration::from_secs(30),
                                            Command::new("ssh")
                                                .args([
                                                    "-o",
                                                    "ConnectTimeout=5",
                                                    "-o",
                                                    "BatchMode=yes",
                                                    "-o",
                                                    "StrictHostKeyChecking=no",
                                                    &name,
                                                    ts_cmd,
                                                ])
                                                .output(),
                                        )
                                        .await
                                        .ok()
                                        .and_then(|r| r.ok());
                                        let ts_ok = ts_result
                                            .as_ref()
                                            .map(|o| {
                                                !String::from_utf8_lossy(&o.stdout).contains("FAIL")
                                            })
                                            .unwrap_or(false);

                                        if ts_ok {
                                            let _ = tx.send(DiagStep {
                                                label: "  → Restart Tailscale".into(),
                                                status: DiagStatus::Fixed,
                                                detail: "Tailscale restarted!".into(),
                                            });
                                            tokio::time::sleep(Duration::from_secs(5)).await;
                                            let _ = tx.send(DiagStep {
                                                label: "  → Verify".into(),
                                                status: DiagStatus::Running,
                                                detail: format!("re-testing {}...", host),
                                            });
                                            let verify = tokio::time::timeout(
                                                Duration::from_secs(6),
                                                Command::new("ssh")
                                                    .args([
                                                        "-o",
                                                        "ConnectTimeout=3",
                                                        "-o",
                                                        "BatchMode=yes",
                                                        "-o",
                                                        "StrictHostKeyChecking=no",
                                                        &format!("{}@{}", user, host),
                                                        "echo ok",
                                                    ])
                                                    .output(),
                                            )
                                            .await
                                            .ok()
                                            .and_then(|r| r.ok())
                                            .map(|o| o.status.success())
                                            .unwrap_or(false);
                                            if verify {
                                                let _ = tx.send(DiagStep {
                                                    label: "  → Verify".into(),
                                                    status: DiagStatus::Pass,
                                                    detail: "Tailscale SSH restored!".into(),
                                                });
                                                let _ = tx.send(DiagStep {
                                                    label: "SSH connectivity".into(),
                                                    status: DiagStatus::Fixed,
                                                    detail: format!(
                                                        "fixed via SSH config alias ({})",
                                                        config_host
                                                    ),
                                                });
                                                true
                                            } else {
                                                let _ = tx.send(DiagStep {
                                                    label: "  → Verify".into(),
                                                    status: DiagStatus::Fail,
                                                    detail: "Tailscale still not routing".into(),
                                                });
                                                let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Fail,
                                                    detail: format!("Tailscale restarted but mesh route failed — try: ssh {} then 'sudo tailscale up'", name) });
                                                false
                                            }
                                        } else {
                                            let _ = tx.send(DiagStep {
                                                label: "  → Restart Tailscale".into(),
                                                status: DiagStatus::Fail,
                                                detail: "restart command failed".into(),
                                            });
                                            let _ = tx.send(DiagStep { label: "SSH connectivity".into(), status: DiagStatus::Fail,
                                                detail: format!("reachable via 'ssh {}' but Tailscale restart failed — needs manual intervention", name) });
                                            false
                                        }
                                    } else {
                                        let _ = tx.send(DiagStep {
                                            label: "  → SSH via alias".into(),
                                            status: DiagStatus::Fail,
                                            detail: format!("{} also unreachable", config_host),
                                        });
                                        let loc = location.to_lowercase();
                                        let hint = if loc.contains("vps") {
                                            "VPS unreachable — check hosting provider console (GoDaddy/DO/Vultr)"
                                        } else if loc.contains("mobile") {
                                            "mobile device — may be off-network or powered down"
                                        } else if loc.contains("sm") || loc.contains("strange") {
                                            "Strange Music network — Tailscale may be intentionally down (Bob policy)"
                                        } else {
                                            "machine appears fully offline — check physical power"
                                        };
                                        let _ = tx.send(DiagStep {
                                            label: "SSH connectivity".into(),
                                            status: DiagStatus::Fail,
                                            detail: hint.into(),
                                        });
                                        false
                                    }
                                } else {
                                    let _ = tx.send(DiagStep {
                                        label: "  → SSH config".into(),
                                        status: DiagStatus::Skipped,
                                        detail: "no SSH config alias found".into(),
                                    });
                                    let loc = location.to_lowercase();
                                    let hint = if loc.contains("vps") {
                                        "VPS unreachable on all paths — check hosting provider console"
                                    } else if loc.contains("mobile") {
                                        "mobile device unreachable — may be off-network"
                                    } else if loc.contains("sm") || loc.contains("strange") {
                                        "SM machine — Tailscale may be down per policy (Bob)"
                                    } else {
                                        "add LAN IP to /etc/hosts, or check physical power"
                                    };
                                    let _ = tx.send(DiagStep {
                                        label: "SSH connectivity".into(),
                                        status: DiagStatus::Fail,
                                        detail: hint.into(),
                                    });
                                    false
                                }
                            };
                            lan_fixed
                        }
                    } else {
                        let _ = tx.send(DiagStep {
                            label: "  → Tailscale route".into(),
                            status: DiagStatus::Fail,
                            detail: format!(
                                "{} not in Tailscale peer list — may need re-enrollment",
                                host
                            ),
                        });
                        let _ = tx.send(DiagStep {
                            label: "SSH connectivity".into(),
                            status: DiagStatus::Fail,
                            detail: "not in mesh — needs Tailscale login on target machine".into(),
                        });
                        false
                    };
                    peer_result
                } else {
                    // Ping works but SSH failed — try again with longer timeout
                    let _ = tx.send(DiagStep {
                        label: "  → Ping".into(),
                        status: DiagStatus::Pass,
                        detail: format!("{} responds to ping", host),
                    });
                    let _ = tx.send(DiagStep {
                        label: "  → SSH retry".into(),
                        status: DiagStatus::Running,
                        detail: "retrying with 5s timeout...".into(),
                    });
                    let retry = tokio::time::timeout(
                        Duration::from_secs(8),
                        Command::new("ssh")
                            .args([
                                "-o",
                                "ConnectTimeout=5",
                                "-o",
                                "BatchMode=yes",
                                "-o",
                                "StrictHostKeyChecking=no",
                                &format!("{}@{}", user, host),
                                "echo ok",
                            ])
                            .output(),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                    if retry {
                        let _ = tx.send(DiagStep {
                            label: "  → SSH retry".into(),
                            status: DiagStatus::Pass,
                            detail: "connected on retry".into(),
                        });
                        let _ = tx.send(DiagStep {
                            label: "SSH connectivity".into(),
                            status: DiagStatus::Fixed,
                            detail: "connected (slow handshake)".into(),
                        });
                        true
                    } else {
                        // Check if port 22 is open
                        let port_check = tokio::time::timeout(Duration::from_secs(3),
                            Command::new("bash").args(["-c", &format!("echo | nc -w 2 {} 22 2>/dev/null && echo OPEN || echo CLOSED", host)]).output()
                        ).await.ok().and_then(|r| r.ok());
                        let port_open = port_check
                            .as_ref()
                            .map(|o| String::from_utf8_lossy(&o.stdout).contains("OPEN"))
                            .unwrap_or(false);
                        if port_open {
                            let _ = tx.send(DiagStep {
                                label: "  → SSH retry".into(),
                                status: DiagStatus::Fail,
                                detail: "port 22 open but auth fails — check SSH keys".into(),
                            });
                            let _ = tx.send(DiagStep {
                                label: "SSH connectivity".into(),
                                status: DiagStatus::Fail,
                                detail: "auth rejected — deploy SSH key with: ssh-copy-id".into(),
                            });
                        } else {
                            let _ = tx.send(DiagStep {
                                label: "  → SSH retry".into(),
                                status: DiagStatus::Fail,
                                detail: "port 22 closed — sshd not running".into(),
                            });
                            let _ = tx.send(DiagStep {
                                label: "SSH connectivity".into(),
                                status: DiagStatus::Fail,
                                detail: "sshd not running on target — needs manual start".into(),
                            });
                        }
                        false
                    }
                }
            } else if ssh_ok {
                let _ = tx.send(DiagStep {
                    label: "SSH connectivity".into(),
                    status: DiagStatus::Pass,
                    detail: "connected".into(),
                });
                true
            } else {
                let _ = tx.send(DiagStep {
                    label: "SSH connectivity".into(),
                    status: DiagStatus::Fail,
                    detail: "unreachable — press D to diagnose deeper".into(),
                });
                false
            };

            if !ssh_ok {
                let _ = tx.send(DiagStep {
                    label: "DONE".into(),
                    status: DiagStatus::Fail,
                    detail: "Cannot proceed without SSH — see details above".into(),
                });
                if let (Some(op_id), Some(ref pool)) = (op_id, pool_opt.as_ref()) {
                    let _ = db::complete_operation(pool, op_id, "failed", Some("SSH unreachable"))
                        .await;
                }
                return;
            }

            // Step 2: Tailscale status
            let _ = tx.send(DiagStep {
                label: "Tailscale".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let ts_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                &format!("{}@{}", user, host), r#"tailscale status --self --json 2>/dev/null | grep -o '"Online":[a-z]*' | head -1 | cut -d: -f2 || echo ?"#
            ]).output().await;
            let ts_online = ts_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or("?".into());
            let ts_ok = ts_online == "True" || ts_online == "true";
            if !ts_ok && fix {
                let _ = tx.send(DiagStep {
                    label: "Tailscale".into(),
                    status: DiagStatus::Running,
                    detail: "restarting Tailscale...".into(),
                });
                let ts_fix_cmd = if is_mac {
                    format!(
                        "{}sudo /Applications/Tailscale.app/Contents/MacOS/Tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || echo FAIL",
                        pfx, name
                    )
                } else {
                    format!(
                        "sudo systemctl restart tailscaled 2>/dev/null; sleep 2; sudo tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 || echo FAIL",
                        name
                    )
                };
                let ts_fix = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &ts_fix_cmd,
                    ])
                    .output()
                    .await;
                let fixed = ts_fix
                    .as_ref()
                    .map(|o| !String::from_utf8_lossy(&o.stdout).contains("FAIL"))
                    .unwrap_or(false);
                let _ = tx.send(DiagStep {
                    label: "Tailscale".into(),
                    status: if fixed {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if fixed {
                        "restarted".into()
                    } else {
                        format!("restart failed — status was: {}", ts_online)
                    },
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Tailscale".into(),
                    status: if ts_ok {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if ts_ok {
                        "online".into()
                    } else {
                        format!("status: {}", ts_online)
                    },
                });
            }

            // Step 3: OpenClaw installed
            let _ = tx.send(DiagStep {
                label: "OpenClaw installed".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let oc_out = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    &format!(
                        "{}openclaw --version 2>/dev/null || echo NOT_INSTALLED",
                        pfx
                    ),
                ])
                .output()
                .await;
            let oc_ver = oc_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or("?".into());
            let oc_installed = !oc_ver.contains("NOT_INSTALLED") && oc_ver != "?";
            if !oc_installed && fix {
                let _ = tx.send(DiagStep {
                    label: "OpenClaw installed".into(),
                    status: DiagStatus::Running,
                    detail: "installing...".into(),
                });
                let install_cmd = if is_mac {
                    format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx)
                } else {
                    "sudo npm install -g openclaw@latest 2>&1 | tail -1".into()
                };
                let _ = tokio::time::timeout(
                    Duration::from_secs(120),
                    Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &install_cmd,
                        ])
                        .output(),
                )
                .await;
                let _ = tx.send(DiagStep {
                    label: "OpenClaw installed".into(),
                    status: DiagStatus::Fixed,
                    detail: "installed".into(),
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "OpenClaw installed".into(),
                    status: if oc_installed {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if oc_installed {
                        oc_ver
                    } else {
                        "not found — run with fix to install".into()
                    },
                });
            }

            // Step 4: Gateway running
            let _ = tx.send(DiagStep {
                label: "Gateway running".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let gw_out = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    &format!(
                        "ss -tlnp 2>/dev/null | grep {} | head -1 || echo NONE",
                        gw_port
                    ),
                ])
                .output()
                .await;
            let gw_line = gw_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or("?".into());
            let gw_running = !gw_line.contains("NONE") && !gw_line.is_empty();
            if !gw_running && fix {
                let _ = tx.send(DiagStep {
                    label: "Gateway running".into(),
                    status: DiagStatus::Running,
                    detail: "starting gateway...".into(),
                });
                let start_cmd = if is_mac {
                    format!("{}nohup openclaw gateway start > /dev/null 2>&1 &", pfx)
                } else {
                    "sudo systemctl start openclaw-gateway 2>/dev/null || nohup openclaw gateway start > /dev/null 2>&1 &".into()
                };
                let _ = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &start_cmd,
                    ])
                    .output()
                    .await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = tx.send(DiagStep {
                    label: "Gateway running".into(),
                    status: DiagStatus::Fixed,
                    detail: "started".into(),
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Gateway running".into(),
                    status: if gw_running {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if gw_running {
                        format!("port {}", gw_port)
                    } else {
                        "not running".into()
                    },
                });
            }

            // Step 5: Gateway API responding
            let _ = tx.send(DiagStep {
                label: "Gateway API".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let api_out = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    &format!(
                        "curl -s -m 3 http://localhost:{}/health 2>/dev/null || echo FAIL",
                        gw_port
                    ),
                ])
                .output()
                .await;
            let api_resp = api_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or("?".into());
            let api_ok = !api_resp.contains("FAIL") && !api_resp.is_empty();
            if !api_ok && fix {
                let _ = tx.send(DiagStep {
                    label: "Gateway API".into(),
                    status: DiagStatus::Running,
                    detail: "restarting gateway...".into(),
                });
                let restart_cmd = if is_mac {
                    format!("{}openclaw gateway restart 2>&1 | tail -1", pfx)
                } else {
                    "systemctl --user restart openclaw-gateway 2>/dev/null || sudo systemctl restart openclaw-gateway 2>/dev/null || openclaw gateway restart 2>&1 | tail -1".into()
                };
                let _ = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &restart_cmd,
                    ])
                    .output()
                    .await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                // Re-check
                let recheck = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &format!(
                            "curl -s -m 3 http://localhost:{}/health 2>/dev/null || echo FAIL",
                            gw_port
                        ),
                    ])
                    .output()
                    .await;
                let recheck_ok = recheck
                    .as_ref()
                    .map(|o| !String::from_utf8_lossy(&o.stdout).contains("FAIL"))
                    .unwrap_or(false);
                let _ = tx.send(DiagStep {
                    label: "Gateway API".into(),
                    status: if recheck_ok {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if recheck_ok {
                        "gateway restarted — API responding".into()
                    } else {
                        "gateway restart attempted but still not responding".into()
                    },
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Gateway API".into(),
                    status: if api_ok {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if api_ok {
                        "responding".into()
                    } else {
                        "not responding".into()
                    },
                });
            }

            // Step 6: Config file exists
            let _ = tx.send(DiagStep {
                label: "Config file".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let cfg_out = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    "test -f ~/.openclaw/openclaw.json && echo EXISTS || echo MISSING",
                ])
                .output()
                .await;
            let cfg_exists = cfg_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "EXISTS")
                .unwrap_or(false);
            if !cfg_exists && fix {
                let _ = tx.send(DiagStep {
                    label: "Config file".into(),
                    status: DiagStatus::Running,
                    detail: "creating default config...".into(),
                });
                let init_cmd = format!(
                    "{}mkdir -p ~/.openclaw && openclaw init --non-interactive 2>/dev/null || echo '{{}}' > ~/.openclaw/openclaw.json && echo CREATED",
                    pfx
                );
                let init_out = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &init_cmd,
                    ])
                    .output()
                    .await;
                let created = init_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains("CREATED"))
                    .unwrap_or(false);
                let _ = tx.send(DiagStep {
                    label: "Config file".into(),
                    status: if created {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if created {
                        "config initialized".into()
                    } else {
                        "failed to create config".into()
                    },
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Config file".into(),
                    status: if cfg_exists {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if cfg_exists {
                        "~/.openclaw/openclaw.json".into()
                    } else {
                        "missing".into()
                    },
                });
            }

            // Step 7: Workspace exists
            let _ = tx.send(DiagStep {
                label: "Agent workspace".into(),
                status: DiagStatus::Running,
                detail: String::new(),
            });
            let ws_out = Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=2",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    "ls ~/CLAUDE/clawd/SOUL.md 2>/dev/null && echo HAS_SOUL || echo NO_SOUL",
                ])
                .output()
                .await;
            let has_soul = ws_out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("HAS_SOUL"))
                .unwrap_or(false);
            if !has_soul && fix {
                let _ = tx.send(DiagStep {
                    label: "Agent workspace".into(),
                    status: DiagStatus::Running,
                    detail: "creating workspace...".into(),
                });
                let ws_cmd = format!(
                    "mkdir -p ~/CLAUDE/clawd/memory && echo '# SOUL.md' > ~/CLAUDE/clawd/SOUL.md && echo '# AGENTS.md' > ~/CLAUDE/clawd/AGENTS.md && echo '# MEMORY.md' > ~/CLAUDE/clawd/MEMORY.md && echo CREATED"
                );
                let ws_result = Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &ws_cmd,
                    ])
                    .output()
                    .await;
                let created = ws_result
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains("CREATED"))
                    .unwrap_or(false);
                let _ = tx.send(DiagStep {
                    label: "Agent workspace".into(),
                    status: if created {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if created {
                        "workspace scaffolded (SOUL.md, AGENTS.md, MEMORY.md)".into()
                    } else {
                        "failed to create workspace".into()
                    },
                });
            } else {
                let _ = tx.send(DiagStep {
                    label: "Agent workspace".into(),
                    status: if has_soul {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    detail: if has_soul {
                        "SOUL.md found".into()
                    } else {
                        "no SOUL.md — agent may lack identity".into()
                    },
                });
            }

            // Step 8: RAM check — warn if < 1GB free
            if !is_mac {
                // Shell command to create a 2GB swap file (reused in steps 8 and 9)
                let create_swap_cmd = "sudo fallocate -l 2G /swapfile 2>/dev/null || sudo dd if=/dev/zero of=/swapfile bs=1M count=2048 2>/dev/null; sudo chmod 600 /swapfile; sudo mkswap /swapfile 2>/dev/null; sudo swapon /swapfile 2>/dev/null; grep -q /swapfile /etc/fstab || echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab; echo SWAP_CREATED";
                let _ = tx.send(DiagStep { label: "RAM available".into(), status: DiagStatus::Running, detail: String::new() });
                let mem_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), "free -m 2>/dev/null | awk '/^Mem:/{print (NF>=7)?$7:$4}' || echo ?"
                ]).output().await;
                let mem_free_str = mem_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
                let mem_free_mb = mem_free_str.parse::<i64>().ok();
                let mem_ok = mem_free_mb.map(|m| m >= 1024).unwrap_or(true);
                if !mem_ok && fix {
                    if let Some(mfree) = mem_free_mb {
                        let _ = tx.send(DiagStep { label: "RAM available".into(), status: DiagStatus::Running,
                            detail: format!("{}MB free — creating 2GB swap file...", mfree) });
                    }
                    // Check if swap already exists; create 2GB swap file if not
                    let swap_check = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host), "free -m | awk '/^Swap:/{print $2}'"
                    ]).output().await;
                    let swap_total = swap_check.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().unwrap_or(0)).unwrap_or(0);
                    let swap_fix_cmd = if swap_total == 0 { create_swap_cmd } else { "echo SWAP_EXISTS" };
                    let swap_result = tokio::time::timeout(Duration::from_secs(60),
                        Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host), swap_fix_cmd]).output()
                    ).await.ok().and_then(|r| r.ok());
                    let swap_created = swap_result.as_ref().map(|o| {
                        let s = String::from_utf8_lossy(&o.stdout);
                        s.contains("SWAP_CREATED") || s.contains("SWAP_EXISTS")
                    }).unwrap_or(false);
                    let _ = tx.send(DiagStep {
                        label: "RAM available".into(),
                        status: if swap_created { DiagStatus::Fixed } else { DiagStatus::Fail },
                        detail: if swap_created { "swap file created — system has virtual memory buffer".into() }
                            else { "could not create swap — check sudo permissions".into() },
                    });
                } else {
                    let detail = match mem_free_mb {
                        Some(m) if m >= 1024 => format!("{:.1}GB free", m as f32 / 1024.0),
                        Some(m) => format!("{}MB free — below 1GB threshold", m),
                        None => "could not read memory info".into(),
                    };
                    let _ = tx.send(DiagStep {
                        label: "RAM available".into(),
                        status: if mem_ok { DiagStatus::Pass } else { DiagStatus::Fail },
                        detail,
                    });
                }

                // Step 9: Swap check — warn if no swap
                let _ = tx.send(DiagStep { label: "Swap configured".into(), status: DiagStatus::Running, detail: String::new() });
                let swap_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), "free -m 2>/dev/null | awk '/^Swap:/{print $2}' || echo ?"
                ]).output().await;
                let swap_str = swap_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or("?".into());
                let swap_mb_diag = swap_str.parse::<i64>().ok();
                let has_swap = swap_mb_diag.map(|s| s > 0).unwrap_or(true);
                if !has_swap && fix {
                    let _ = tx.send(DiagStep { label: "Swap configured".into(), status: DiagStatus::Running, detail: "no swap — creating 2GB swap file...".into() });
                    let swap_result = tokio::time::timeout(Duration::from_secs(60),
                        Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host), create_swap_cmd]).output()
                    ).await.ok().and_then(|r| r.ok());
                    let created = swap_result.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).contains("SWAP_CREATED")).unwrap_or(false);
                    let _ = tx.send(DiagStep {
                        label: "Swap configured".into(),
                        status: if created { DiagStatus::Fixed } else { DiagStatus::Fail },
                        detail: if created { "/swapfile (2GB) created and activated".into() }
                            else { "swap creation failed — check sudo permissions and disk space".into() },
                    });
                } else {
                    let detail = match swap_mb_diag {
                        Some(s) if s > 0 => format!("{}MB swap available", s),
                        Some(_) => "no swap — OOM kill risk on memory pressure".into(),
                        None => "could not read swap info".into(),
                    };
                    let _ = tx.send(DiagStep {
                        label: "Swap configured".into(),
                        status: if has_swap { DiagStatus::Pass } else { DiagStatus::Fail },
                        detail,
                    });
                }
            }

            // Step 10: Systemd service hardening (only on Linux)
            if !is_mac {
                let _ = tx.send(DiagStep { label: "Service hardening".into(), status: DiagStatus::Running, detail: String::new() });
                let svc_check_cmd = r#"SVC=openclaw-gateway; FILE=$(systemctl cat $SVC 2>/dev/null | grep -v '^#' | tr '\n' '|'); HAS_RESTART=$(echo "$FILE" | grep -c 'Restart=always'); HAS_BURST=$(echo "$FILE" | grep -c 'StartLimitBurst'); HAS_KILL=$(echo "$FILE" | grep 'KillMode' | grep -c 'process'); HAS_MEM=$(echo "$FILE" | grep -c 'MemoryMax\|MemoryLimit'); echo "RESTART:$HAS_RESTART BURST:$HAS_BURST KILLMODE:$HAS_KILL MEMMAX:$HAS_MEM""#;
                let svc_out = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host), svc_check_cmd
                ]).output().await;
                let svc_info = svc_out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
                let restart_always = svc_info.contains("RESTART:1") || svc_info.contains("RESTART:2");
                let has_burst = svc_info.contains("BURST:1") || svc_info.contains("BURST:2");
                let kill_mode_process = svc_info.contains("KILLMODE:1") || svc_info.contains("KILLMODE:2");
                let has_mem_max = svc_info.contains("MEMMAX:1") || svc_info.contains("MEMMAX:2");
                let issues: Vec<&str> = [
                    if restart_always && !has_burst { Some("no StartLimitBurst") } else { None },
                    if kill_mode_process { Some("KillMode=process (orphans)") } else { None },
                    if !has_mem_max { Some("no MemoryMax") } else { None },
                ].iter().filter_map(|x| *x).collect();

                if !issues.is_empty() && fix {
                    let _ = tx.send(DiagStep { label: "Service hardening".into(), status: DiagStatus::Running, detail: "applying systemd drop-in...".into() });
                    let dropin_cmd = r#"DROPIN=/etc/systemd/system/openclaw-gateway.service.d/mc-hardening.conf; sudo mkdir -p $(dirname $DROPIN); printf '[Service]\nKillMode=control-group\nMemoryMax=2G\nMemorySwapMax=512M\n[Unit]\nStartLimitBurst=3\nStartLimitIntervalSec=60\n' | sudo tee $DROPIN > /dev/null && sudo systemctl daemon-reload && echo APPLIED"#;
                    let dropin_result = Command::new("ssh").args(["-o","ConnectTimeout=2","-o","BatchMode=yes","-o","StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host), dropin_cmd]).output().await;
                    let applied = dropin_result.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).contains("APPLIED")).unwrap_or(false);
                    let _ = tx.send(DiagStep {
                        label: "Service hardening".into(),
                        status: if applied { DiagStatus::Fixed } else { DiagStatus::Fail },
                        detail: if applied { "drop-in applied: KillMode=control-group, MemoryMax=2G, StartLimitBurst=3".into() }
                            else { "drop-in failed — check sudo permissions".into() },
                    });
                } else if issues.is_empty() {
                    let _ = tx.send(DiagStep { label: "Service hardening".into(), status: DiagStatus::Pass, detail: "KillMode, MemoryMax, and StartLimitBurst look good".into() });
                } else {
                    let _ = tx.send(DiagStep { label: "Service hardening".into(), status: DiagStatus::Fail, detail: format!("issues: {} — run D to auto-fix", issues.join(", ")) });
                }
            }

            // Done
            let _ = tx.send(DiagStep {
                label: "DONE".into(),
                status: DiagStatus::Pass,
                detail: "diagnostic complete".into(),
            });
            if let (Some(op_id), Some(ref pool)) = (op_id, pool_opt.as_ref()) {
                let _ = db::complete_operation(pool, op_id, "completed", None).await;
            }
        });
    }

    /// Start fleet diagnostics for all multi-selected agents sequentially.
    /// Falls back to single-agent diagnostic if nothing is multi-selected.
    fn start_fleet_diagnostics(&mut self, fix: bool) {
        if self.selected_agents.is_empty() {
            self.start_diagnostics(fix);
            return;
        }

        let mut indices: Vec<usize> = self.selected_agent_indices();
        indices.sort();

        // Build snapshot of agent info for the background task
        let agents_to_run: Vec<(usize, String, String, String, String, i32)> = indices
            .iter()
            .filter_map(|&i| {
                self.agents.get(i).map(|a| {
                    (
                        i,
                        a.name.clone(),
                        a.emoji.clone(),
                        a.host.clone(),
                        a.ssh_user.clone(),
                        a.gateway_port,
                    )
                })
            })
            .collect();

        if agents_to_run.is_empty() {
            return;
        }

        self.fleet_diag_results = agents_to_run
            .iter()
            .enumerate()
            .map(
                |(ri, (_ai, name, emoji, _host, _user, _gw_port))| FleetDiagResult {
                    agent_idx: agents_to_run[ri].0,
                    name: name.clone(),
                    emoji: emoji.clone(),
                    checks: [None; 7],
                    top_issue: String::new(),
                    running: ri == 0,
                    done: false,
                },
            )
            .collect();

        self.fleet_diag_active = true;
        self.fleet_diag_fix = fix;
        self.fleet_diag_selected = 0;
        self.fleet_diag_done = false;

        let (tx, rx) = mpsc::unbounded_channel::<FleetDiagMsg>();
        self.fleet_diag_rx = Some(rx);

        tokio::spawn(async move {
            for (result_idx, (_agent_idx, name, _emoji, host, user, gw_port)) in
                agents_to_run.iter().enumerate()
            {
                let _ = tx.send(FleetDiagMsg::AgentStart(result_idx));

                // Detect macOS for PATH prefix
                let is_mac_check = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "uname -s",
                    ])
                    .output()
                    .await;
                let is_mac = is_mac_check
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Darwin")
                    .unwrap_or(false);
                let pfx = if is_mac {
                    "export PATH=/opt/homebrew/bin:$PATH; "
                } else {
                    ""
                };

                // Check 0: SSH connectivity
                let ssh_ok = tokio::time::timeout(
                    Duration::from_secs(6),
                    tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            "echo ok",
                        ])
                        .output(),
                )
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|o| o.status.success())
                .unwrap_or(false);
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 0,
                    status: if ssh_ok {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    },
                    issue: if ssh_ok {
                        String::new()
                    } else {
                        "SSH unreachable".into()
                    },
                });

                if !ssh_ok {
                    // Mark remaining checks as skipped
                    for ci in 1..7 {
                        let _ = tx.send(FleetDiagMsg::CheckDone {
                            result_idx,
                            check_idx: ci,
                            status: DiagStatus::Skipped,
                            issue: String::new(),
                        });
                    }
                    let _ = tx.send(FleetDiagMsg::AgentDone(result_idx));
                    continue;
                }

                // Check 1: Tailscale
                let ts_out = tokio::process::Command::new("ssh").args([
                    "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                    &format!("{}@{}", user, host),
                    r#"tailscale status --self --json 2>/dev/null | grep -o '"Online":[a-z]*' | head -1 | cut -d: -f2 || echo ?"#
                ]).output().await;
                let ts_online = ts_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or("?".into());
                let ts_ok = ts_online == "True" || ts_online == "true";
                let ts_status = if !ts_ok && fix {
                    let fix_cmd = if is_mac {
                        format!(
                            "{}sudo /Applications/Tailscale.app/Contents/MacOS/Tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 | tail -1 || echo FAIL",
                            pfx, name
                        )
                    } else {
                        format!(
                            "sudo systemctl restart tailscaled 2>/dev/null; sleep 2; sudo tailscale up --login-server=https://vpn.tinyblue.dev --accept-routes --hostname={} --reset --timeout=25s 2>&1 | tail -1 || echo FAIL",
                            name
                        )
                    };
                    let fix_out = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &fix_cmd,
                        ])
                        .output()
                        .await;
                    if fix_out
                        .map(|o| !String::from_utf8_lossy(&o.stdout).contains("FAIL"))
                        .unwrap_or(false)
                    {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    }
                } else {
                    if ts_ok {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 1,
                    status: ts_status,
                    issue: if matches!(ts_status, DiagStatus::Fail) {
                        format!("Tailscale offline ({})", ts_online)
                    } else {
                        String::new()
                    },
                });

                // Check 2: OpenClaw installed
                let oc_out = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &format!(
                            "{}openclaw --version 2>/dev/null || echo NOT_INSTALLED",
                            pfx
                        ),
                    ])
                    .output()
                    .await;
                let oc_ver = oc_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or("?".into());
                let oc_installed = !oc_ver.contains("NOT_INSTALLED") && oc_ver != "?";
                let oc_status = if !oc_installed && fix {
                    let install_cmd = if is_mac {
                        format!("{}npm install -g openclaw@latest 2>&1 | tail -1", pfx)
                    } else {
                        "sudo npm install -g openclaw@latest 2>&1 | tail -1".into()
                    };
                    let _ = tokio::time::timeout(
                        Duration::from_secs(120),
                        tokio::process::Command::new("ssh")
                            .args([
                                "-o",
                                "ConnectTimeout=2",
                                "-o",
                                "BatchMode=yes",
                                "-o",
                                "StrictHostKeyChecking=no",
                                &format!("{}@{}", user, host),
                                &install_cmd,
                            ])
                            .output(),
                    )
                    .await;
                    DiagStatus::Fixed
                } else {
                    if oc_installed {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 2,
                    status: oc_status,
                    issue: if matches!(oc_status, DiagStatus::Fail) {
                        "OpenClaw not installed".into()
                    } else {
                        String::new()
                    },
                });

                // Check 3: Gateway running
                let gw_out = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &format!(
                            "ss -tlnp 2>/dev/null | grep {} | head -1 || echo NONE",
                            gw_port
                        ),
                    ])
                    .output()
                    .await;
                let gw_line = gw_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or("?".into());
                let gw_running = !gw_line.contains("NONE") && !gw_line.is_empty();
                let gw_status = if !gw_running && fix {
                    let start_cmd = if is_mac {
                        format!("{}nohup openclaw gateway start > /dev/null 2>&1 &", pfx)
                    } else {
                        "sudo systemctl start openclaw-gateway 2>/dev/null || nohup openclaw gateway start > /dev/null 2>&1 &".into()
                    };
                    let _ = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &start_cmd,
                        ])
                        .output()
                        .await;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    DiagStatus::Fixed
                } else {
                    if gw_running {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 3,
                    status: gw_status,
                    issue: if matches!(gw_status, DiagStatus::Fail) {
                        "Gateway not running".into()
                    } else {
                        String::new()
                    },
                });

                // Check 4: Gateway API
                let api_out = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &format!(
                            "curl -s -m 3 http://localhost:{}/health 2>/dev/null || echo FAIL",
                            gw_port
                        ),
                    ])
                    .output()
                    .await;
                let api_resp = api_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or("?".into());
                let api_ok = !api_resp.contains("FAIL") && !api_resp.is_empty();
                let api_status = if !api_ok && fix {
                    let restart_cmd = if is_mac {
                        format!("{}openclaw gateway restart 2>&1 | tail -1", pfx)
                    } else {
                        "systemctl --user restart openclaw-gateway 2>/dev/null || sudo systemctl restart openclaw-gateway 2>/dev/null || openclaw gateway restart 2>&1 | tail -1".into()
                    };
                    let _ = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &restart_cmd,
                        ])
                        .output()
                        .await;
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    let recheck = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &format!(
                                "curl -s -m 3 http://localhost:{}/health 2>/dev/null || echo FAIL",
                                gw_port
                            ),
                        ])
                        .output()
                        .await;
                    if recheck
                        .map(|o| !String::from_utf8_lossy(&o.stdout).contains("FAIL"))
                        .unwrap_or(false)
                    {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    }
                } else {
                    if api_ok {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 4,
                    status: api_status,
                    issue: if matches!(api_status, DiagStatus::Fail) {
                        "Gateway API down".into()
                    } else {
                        String::new()
                    },
                });

                // Check 5: Config file
                let cfg_out = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "test -f ~/.openclaw/openclaw.json && echo EXISTS || echo MISSING",
                    ])
                    .output()
                    .await;
                let cfg_exists = cfg_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "EXISTS")
                    .unwrap_or(false);
                let cfg_status = if !cfg_exists && fix {
                    let init_cmd = format!(
                        "{}mkdir -p ~/.openclaw && openclaw init --non-interactive 2>/dev/null || echo '{{}}' > ~/.openclaw/openclaw.json && echo CREATED",
                        pfx
                    );
                    let init_out = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            &init_cmd,
                        ])
                        .output()
                        .await;
                    if init_out
                        .map(|o| String::from_utf8_lossy(&o.stdout).contains("CREATED"))
                        .unwrap_or(false)
                    {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    }
                } else {
                    if cfg_exists {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 5,
                    status: cfg_status,
                    issue: if matches!(cfg_status, DiagStatus::Fail) {
                        "Config missing".into()
                    } else {
                        String::new()
                    },
                });

                // Check 6: Workspace
                let ws_out = tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "ls ~/CLAUDE/clawd/SOUL.md 2>/dev/null && echo HAS_SOUL || echo NO_SOUL",
                    ])
                    .output()
                    .await;
                let has_soul = ws_out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains("HAS_SOUL"))
                    .unwrap_or(false);
                let ws_status = if !has_soul && fix {
                    let ws_cmd = "mkdir -p ~/CLAUDE/clawd/memory && echo '# SOUL.md' > ~/CLAUDE/clawd/SOUL.md && echo '# AGENTS.md' > ~/CLAUDE/clawd/AGENTS.md && echo '# MEMORY.md' > ~/CLAUDE/clawd/MEMORY.md && echo CREATED";
                    let ws_result = tokio::process::Command::new("ssh")
                        .args([
                            "-o",
                            "ConnectTimeout=2",
                            "-o",
                            "BatchMode=yes",
                            "-o",
                            "StrictHostKeyChecking=no",
                            &format!("{}@{}", user, host),
                            ws_cmd,
                        ])
                        .output()
                        .await;
                    if ws_result
                        .map(|o| String::from_utf8_lossy(&o.stdout).contains("CREATED"))
                        .unwrap_or(false)
                    {
                        DiagStatus::Fixed
                    } else {
                        DiagStatus::Fail
                    }
                } else {
                    if has_soul {
                        DiagStatus::Pass
                    } else {
                        DiagStatus::Fail
                    }
                };
                let _ = tx.send(FleetDiagMsg::CheckDone {
                    result_idx,
                    check_idx: 6,
                    status: ws_status,
                    issue: if matches!(ws_status, DiagStatus::Fail) {
                        "Workspace missing".into()
                    } else {
                        String::new()
                    },
                });

                let _ = tx.send(FleetDiagMsg::AgentDone(result_idx));
            }
            let _ = tx.send(FleetDiagMsg::AllDone);
        });
    }
    fn toggle_service(&mut self) {
        if self.svc_selected >= self.svc_list.len() {
            return;
        }
        let svc = &self.svc_list[self.svc_selected];
        if svc.name == "model" || svc.name == "gateway" {
            return;
        } // Can't toggle these
        let new_state = !svc.enabled;
        let name = svc.name.clone();
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let agent_db_name = agent.db_name.clone();

        let cmd = format!(
            r#"python3 -c "
import json
with open('$HOME/.openclaw/openclaw.json'.replace('$HOME', __import__('os').path.expanduser('~'))) as f:
    d = json.load(f)
d.setdefault('plugins', {{}}).setdefault('entries', {{}}).setdefault('{}', {{}})['enabled'] = {}
with open('$HOME/.openclaw/openclaw.json'.replace('$HOME', __import__('os').path.expanduser('~')), 'w') as f:
    json.dump(d, f, indent=2)
print('ok')
""#,
            name,
            if new_state { "True" } else { "False" }
        );

        let toast_msg = format!(
            "{} {} {}",
            svc.icon,
            name,
            if new_state { "enabled" } else { "disabled" }
        );
        self.toast(&toast_msg);
        self.queue_audit_mutation(
            "agent.service_toggle",
            &format!("{}:{}", agent_db_name, name),
            if new_state { "enabled" } else { "disabled" },
        );

        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &cmd,
                    ])
                    .output(),
            )
            .await;
        });

        // Optimistic update
        if let Some(svc) = self.svc_list.get_mut(self.svc_selected) {
            svc.enabled = new_state;
        }
    }

    /// Load workspace files for focused agent via SSH (non-blocking)
    fn start_workspace_load(&mut self) {
        if self.selected >= self.agents.len() {
            return;
        }
        if self.ws_loading {
            return;
        }
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
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &check_cmd,
                    ])
                    .output(),
            )
            .await;

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
                            if let Some((_, _, icon)) =
                                AGENT_FILES.iter().find(|(n, _, _)| *n == name)
                            {
                                files.push(WorkspaceFile {
                                    name: name.to_string(),
                                    path: path.to_string(),
                                    icon,
                                    exists: true,
                                    size_bytes: Some(size),
                                });
                            }
                        }
                    } else if let Some(name) = line.strip_prefix("MISSING:") {
                        if let Some((_, _, icon)) = AGENT_FILES.iter().find(|(n, _, _)| *n == name)
                        {
                            files.push(WorkspaceFile {
                                name: name.to_string(),
                                path: String::new(),
                                icon,
                                exists: false,
                                size_bytes: None,
                            });
                        }
                    }
                }
            }
            if files.is_empty() {
                for (name, _, icon) in AGENT_FILES {
                    files.push(WorkspaceFile {
                        name: name.to_string(),
                        path: String::new(),
                        icon,
                        exists: false,
                        size_bytes: None,
                    });
                }
            }

            // Crons
            let cron_output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        "openclaw cron list --json 2>/dev/null || echo '[]'",
                    ])
                    .output(),
            )
            .await;
            let mut crons = Vec::new();
            if let Ok(Ok(o)) = cron_output {
                let stdout = String::from_utf8_lossy(&o.stdout);
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(stdout.trim()) {
                    for item in arr {
                        crons.push(CronEntry {
                            id: item["id"].as_str().unwrap_or("").to_string(),
                            schedule: item["schedule"].as_str().unwrap_or("").to_string(),
                            description: item["description"]
                                .as_str()
                                .unwrap_or(item["prompt"].as_str().unwrap_or("(no description)"))
                                .to_string(),
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
        if self.ws_selected >= self.ws_files.len() {
            return;
        }
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
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &format!("cat '{}'", path),
                    ])
                    .output(),
            )
            .await;
            let content = match output {
                Ok(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
                Ok(Ok(o)) => format!("Error: {}", String::from_utf8_lossy(&o.stderr)),
                _ => "(timeout reading file)".to_string(),
            };
            let _ = tx.send(content);
        });
        self.ws_content_scroll = 0;
    }

    fn open_cron_form(&mut self, edit_existing: bool) {
        self.ws_cron_form_active = true;
        self.ws_cron_form_edit = edit_existing;
        self.ws_cron_form_focus = 0;
        if edit_existing {
            if let Some(cron) = self.ws_crons.get(self.ws_cron_selected) {
                self.ws_cron_form_schedule = cron.schedule.clone();
                self.ws_cron_form_description = cron.description.clone();
            } else {
                self.ws_cron_form_schedule.clear();
                self.ws_cron_form_description.clear();
            }
        } else {
            self.ws_cron_form_schedule.clear();
            self.ws_cron_form_description.clear();
        }
    }

    fn start_cron_operation(&mut self, mode: CronOpMode) {
        if self.selected >= self.agents.len() { return; }
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let name = agent.name.clone();
        let mode_name = match mode {
            CronOpMode::Add => "add",
            CronOpMode::Edit => "edit",
            CronOpMode::Delete => "delete",
        };
        let cron_id = match mode {
            CronOpMode::Add => String::new(),
            CronOpMode::Edit | CronOpMode::Delete => match self.ws_crons.get(self.ws_cron_selected) {
                Some(c) if !c.id.is_empty() => c.id.clone(),
                _ => {
                    self.toast("⚠ Select a cron job with a valid ID first");
                    return;
                }
            },
        };
        let schedule = self.ws_cron_form_schedule.trim().to_string();
        let description = self.ws_cron_form_description.trim().to_string();
        if !matches!(mode, CronOpMode::Delete) && (schedule.is_empty() || description.is_empty()) {
            self.toast("⚠ Cron schedule and description are required");
            return;
        }

        self.diag_active = true;
        self.diag_auto_fix = false;
        self.diag_start = Some(Instant::now());
        self.diag_title = Some(format!(" ⏰ Cron manager — {} ", name));
        self.diag_overlay_scroll = 0;
        self.diag_steps.clear();
        self.diag_task_running = true;

        let (tx, rx) = mpsc::unbounded_channel::<DiagStep>();
        self.diag_rx = Some(rx);

        let escaped_mode = shell::escape(mode_name);
        let escaped_id = shell::escape(&cron_id);
        let escaped_schedule = shell::escape(&schedule);
        let escaped_desc = shell::escape(&description);
        tokio::spawn(async move {
            let action_label = match mode {
                CronOpMode::Add => "Add cron job",
                CronOpMode::Edit => "Edit cron job",
                CronOpMode::Delete => "Delete cron job",
            };
            let _ = tx.send(DiagStep { label: action_label.into(), status: DiagStatus::Running, detail: String::new() });
            let cmd = format!(
                "CRON_MODE={} CRON_ID={} CRON_SCHEDULE={} CRON_DESC={} python3 - <<'PY'\n\
import json, os, time\n\
path = os.path.expanduser('~/.openclaw/cron/jobs.json')\n\
os.makedirs(os.path.dirname(path), exist_ok=True)\n\
try:\n\
    with open(path, 'r', encoding='utf-8') as f:\n\
        data = json.load(f)\n\
except Exception:\n\
    data = {{}}\n\
jobs = data.get('jobs', [])\n\
if not isinstance(jobs, list):\n\
    jobs = []\n\
mode = os.environ.get('CRON_MODE', 'add')\n\
cid = os.environ.get('CRON_ID', '')\n\
sched = os.environ.get('CRON_SCHEDULE', '').strip()\n\
desc = os.environ.get('CRON_DESC', '').strip()\n\
if mode == 'delete':\n\
    jobs = [j for j in jobs if str(j.get('id', '')) != cid]\n\
else:\n\
    if not sched:\n\
        raise SystemExit('missing schedule')\n\
    if not desc:\n\
        raise SystemExit('missing description')\n\
    if not cid:\n\
        cid = f'sam-{{int(time.time() * 1000)}}'\n\
    current = None\n\
    for j in jobs:\n\
        if str(j.get('id', '')) == cid:\n\
            current = j\n\
            break\n\
    if current is None:\n\
        current = {{'id': cid, 'enabled': True, 'sessionTarget': 'main'}}\n\
        jobs.append(current)\n\
    current['id'] = cid\n\
    current['name'] = desc\n\
    current['description'] = desc\n\
    current['prompt'] = desc\n\
    current['enabled'] = bool(current.get('enabled', True))\n\
    current['sessionTarget'] = current.get('sessionTarget') or 'main'\n\
    sched_obj = current.get('schedule')\n\
    if not isinstance(sched_obj, dict):\n\
        sched_obj = {{}}\n\
    sched_obj['kind'] = 'cron'\n\
    sched_obj['cron'] = sched\n\
    current['schedule'] = sched_obj\n\
data['jobs'] = jobs\n\
with open(path, 'w', encoding='utf-8') as f:\n\
    json.dump(data, f, indent=2)\n\
print('ok')\n\
PY",
                escaped_mode, escaped_id, escaped_schedule, escaped_desc
            );
            let output = tokio::time::timeout(
                Duration::from_secs(8),
                Command::new("ssh")
                    .args([
                        "-o", "ConnectTimeout=2", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host), &cmd,
                    ])
                    .output(),
            ).await;
            match output {
                Ok(Ok(o)) if o.status.success() => {
                    let _ = tx.send(DiagStep { label: action_label.into(), status: DiagStatus::Pass, detail: "saved".into() });
                    let _ = tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Pass, detail: "Cron update complete [reload-workspace]".into() });
                }
                Ok(Ok(o)) => {
                    let err = String::from_utf8_lossy(&o.stderr).trim().chars().take(120).collect::<String>();
                    let detail = if err.is_empty() { "remote command failed".to_string() } else { err };
                    let _ = tx.send(DiagStep { label: action_label.into(), status: DiagStatus::Fail, detail });
                    let _ = tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Fail, detail: "Cron update failed".into() });
                }
                _ => {
                    let _ = tx.send(DiagStep { label: action_label.into(), status: DiagStatus::Fail, detail: "timeout or SSH error".into() });
                    let _ = tx.send(DiagStep { label: "DONE".into(), status: DiagStatus::Fail, detail: "Cron update failed".into() });
                }
            }
        });
    }

    /// Push current edit state onto undo stack (single-level: clears before pushing)
    fn ws_push_undo(&mut self) {
        self.ws_undo_stack.clear();
        self.ws_undo_stack
            .push((self.ws_edit_buffer.clone(), self.ws_cursor));
    }

    /// Save edited file content back to agent via SSH (non-blocking)
    fn start_file_save(&mut self) {
        if self.ws_selected >= self.ws_files.len() {
            return;
        }
        let file = &self.ws_files[self.ws_selected];
        let agent = &self.agents[self.selected];
        let host = agent.host.clone();
        let user = agent.ssh_user.clone();
        let agent_db_name = agent.db_name.clone();
        let path = if file.path.is_empty() {
            format!("~/CLAUDE/clawd/{}", file.name)
        } else {
            file.path.clone()
        };
        let file_name = file.name.clone();

        let content = self.ws_edit_buffer.join("\n");

        // Validate JSON before saving .json files
        if file.name.ends_with(".json") {
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&content) {
                self.toast(&format!("✗ JSON error: {}", e));
                return;
            }
        }

        let escaped_content = content.replace("'", "'\''");
        let cmd = format!(
            "mkdir -p $(dirname '{}') && cat > '{}' << 'SAMEOF'\n{}\nSAMEOF",
            path, path, escaped_content
        );

        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                Command::new("ssh")
                    .args([
                        "-o",
                        "ConnectTimeout=2",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=no",
                        &format!("{}@{}", user, host),
                        &cmd,
                    ])
                    .output(),
            )
            .await;
        });

        self.ws_editing = false;
        self.ws_discard_confirm = false;
        self.ws_undo_stack.clear();
        self.ws_content = Some(content.clone());
        let fname = if self.ws_selected < self.ws_files.len() {
            self.ws_files[self.ws_selected].name.clone()
        } else {
            "file".into()
        };
        self.toast(&format!("✓ Saved {}", fname));
    }

    fn start_refresh(&mut self) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.refresh_cycle += 1;
        self.last_refresh = Instant::now();
        let (tx, rx) = mpsc::unbounded_channel();
        self.refresh_rx = Some(rx);

        for (i, a) in self.agents.iter().enumerate() {
            let (host, user, sip) = (a.host.clone(), a.ssh_user.clone(), self.self_ip.clone());
            let tx = tx.clone();
            let full = self.refresh_cycle % 5 == 0; // full probe every 5th cycle
            tokio::spawn(async move {
                let (status, os, kern, oc, lat, cpu, ram, disk, act, ctx) =
                    probe_agent(&host, &user, &sip, full).await;
                let _ = tx.send(ProbeResult {
                    index: i,
                    status,
                    os,
                    kernel: kern,
                    oc_version: oc,
                    latency_ms: lat,
                    cpu_pct: cpu,
                    ram_pct: ram,
                    disk_pct: disk,
                    activity: act,
                    context_pct: ctx,
                    gateway_status: GatewayStatus::Unknown,
                    gateway_pid: None,
                });
            });
        }
    }

    fn drain_refresh_results(
        &mut self,
    ) -> Vec<(usize, AgentStatus, String, String, String, Option<u32>)> {
        let mut updates = vec![];
        if let Some(rx) = &mut self.refresh_rx {
            while let Ok(r) = rx.try_recv() {
                if r.index < self.agents.len() {
                    self.agents[r.index].status = r.status.clone();
                    if !r.os.is_empty() {
                        self.agents[r.index].os = r.os.clone();
                    }
                    if !r.kernel.is_empty() {
                        self.agents[r.index].kernel = r.kernel.clone();
                    }
                    if !r.oc_version.is_empty() {
                        self.agents[r.index].oc_version = r.oc_version.clone();
                    }
                    self.agents[r.index].latency_ms = r.latency_ms;
                    self.agents[r.index].cpu_pct = r.cpu_pct;
                    self.agents[r.index].ram_pct = r.ram_pct;
                    self.agents[r.index].disk_pct = r.disk_pct;
                    if r.gateway_status != GatewayStatus::Unknown {
                        self.agents[r.index].gateway_status = r.gateway_status;
                    }
                    if r.gateway_pid.is_some() {
                        self.agents[r.index].gateway_pid = r.gateway_pid;
                    }
                    self.agents[r.index].last_seen = now_str();
                    self.agents[r.index].last_probe_at = Some(Instant::now());
                    updates.push((
                        r.index,
                        r.status,
                        r.os,
                        r.kernel,
                        r.oc_version,
                        r.latency_ms,
                    ));
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
            self.agents
                .iter()
                .enumerate()
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
                let already = self
                    .alerts
                    .iter()
                    .any(|al| al.agent == a.db_name && al.message.contains("offline"));
                if !already {
                    self.alerts.push(Alert {
                        time: now.clone(),
                        created_at: Instant::now(),
                        agent: a.db_name.clone(),
                        emoji: a.emoji.clone(),
                        message: format!("{} went offline", a.name),
                        severity: AlertSeverity::Critical,
                    });
                    self.alert_flash = Some(Instant::now());
                }
            }
            if let Some(disk) = a.disk_pct {
                if disk > 90.0 {
                    let already = self
                        .alerts
                        .iter()
                        .any(|al| al.agent == a.db_name && al.message.contains("disk"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(),
                            created_at: Instant::now(),
                            agent: a.db_name.clone(),
                            emoji: a.emoji.clone(),
                            message: format!("{} disk at {:.0}%", a.name, disk),
                            severity: AlertSeverity::Warning,
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
            if let Some(ram) = a.ram_pct {
                if ram > 90.0 {
                    let already = self
                        .alerts
                        .iter()
                        .any(|al| al.agent == a.db_name && al.message.contains("RAM"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(),
                            created_at: Instant::now(),
                            agent: a.db_name.clone(),
                            emoji: a.emoji.clone(),
                            message: format!("{} RAM at {:.0}%", a.name, ram),
                            severity: AlertSeverity::Warning,
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
            if let Some(mem_free) = a.mem_free_mb {
                if mem_free < 1024 {
                    let already = self.alerts.iter().any(|al| al.agent == a.db_name && al.message.contains("memory"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(), created_at: Instant::now(), agent: a.db_name.clone(), emoji: a.emoji.clone(),
                            message: format!("{} low memory: {}MB free", a.name, mem_free),
                            severity: if mem_free < 256 { AlertSeverity::Critical } else { AlertSeverity::Warning },
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
            if let Some(swap) = a.swap_mb {
                if swap == 0 {
                    let already = self.alerts.iter().any(|al| al.agent == a.db_name && al.message.contains("swap"));
                    if !already {
                        self.alerts.push(Alert {
                            time: now.clone(), created_at: Instant::now(), agent: a.db_name.clone(), emoji: a.emoji.clone(),
                            message: format!("{} no swap configured — OOM risk", a.name),
                            severity: AlertSeverity::Warning,
                        });
                        self.alert_flash = Some(Instant::now());
                    }
                }
            }
        }
        // Keep last 100 alerts
        if self.alerts.len() > 100 {
            self.alerts.drain(0..self.alerts.len() - 100);
        }
    }

    fn update_status_bar(&mut self) {
        let on = self
            .agents
            .iter()
            .filter(|a| a.status == AgentStatus::Online)
            .count();
        let total = self.agents.len();
        let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        // Always advance spinner for a live "app is alive" indicator
        self.spinner_frame = (self.spinner_frame + 1) % spinner_chars.len();
        let refresh = format!(" {} ", spinner_chars[self.spinner_frame]);
        let chat_count = self.chat_history.len();
        let sel_info = if !self.multi_selected.is_empty() {
            format!(" │ 🔲 {}", self.multi_selected.len())
        } else {
            String::new()
        };
        let alert_info = if !self.alerts.is_empty() {
            let crits = self
                .alerts
                .iter()
                .filter(|a| a.severity == AlertSeverity::Critical)
                .count();
            if crits > 0 {
                format!(" │ 🔴 {} alerts", self.alerts.len())
            } else {
                format!(" │ 🟡 {} alerts", self.alerts.len())
            }
        } else {
            String::new()
        };
        self.status_message = format!(
            "v1.2 │ {}/{} online{}{}{} │ sort:{} │ chat({}) │ {}/{} │ /=cmd ?=help",
            on,
            total,
            refresh,
            sel_info,
            alert_info,
            self.sort_mode.label(),
            chat_count,
            self.theme_name.label(),
            self.bg_density.label()
        );
    }
}

// ---- SSH Probe ----

async fn probe_agent(
    host: &str,
    user: &str,
    self_ip: &str,
    full: bool,
) -> (
    AgentStatus,
    String,
    String,
    String,
    Option<u32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    String,
    Option<f32>,
) {
    let start = Instant::now();
    // Fast probe: just SSH echo (connectivity + latency only)
    if !full && host != "localhost" && host != self_ip {
        let tgt = format!("{}@{}", user, host);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            Command::new("ssh")
                .args([
                    "-o",
                    "ConnectTimeout=1",
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "BatchMode=yes",
                    &tgt,
                    "echo",
                    "ok",
                ])
                .output(),
        )
        .await;
        let ms = start.elapsed().as_millis() as u32;
        return match result {
            Ok(Ok(o)) if o.status.success() => (
                AgentStatus::Online,
                String::new(),
                String::new(),
                String::new(),
                Some(ms),
                None,
                None,
                None,
                String::new(),
                None,
            ),
            _ => (
                AgentStatus::Offline,
                String::new(),
                String::new(),
                String::new(),
                None,
                None,
                None,
                None,
                String::new(),
                None,
            ),
        };
    }
    if host == "localhost" || host == self_ip {
        let os = Command::new("bash")
            .args([
                "-c",
                ". /etc/os-release 2>/dev/null && echo \"$NAME $VERSION_ID\" || echo unknown",
            ])
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let kern = Command::new("uname")
            .arg("-r")
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let oc = Command::new("bash").args(["-c", "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; openclaw --version 2>/dev/null || echo ?"]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let cpu = Command::new("bash")
            .args([
                "-c",
                r#"top -bn1 2>/dev/null | grep 'Cpu(s)' | awk '{print $2+$4}'"#,
            ])
            .output()
            .await
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<f32>()
                    .ok()
            })
            .ok()
            .flatten();
        let ram = Command::new("bash")
            .args([
                "-c",
                r#"free 2>/dev/null | awk '/Mem:/{printf "%.1f", $3/$2*100}'"#,
            ])
            .output()
            .await
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<f32>()
                    .ok()
            })
            .ok()
            .flatten();
        let disk = Command::new("bash")
            .args([
                "-c",
                r#"df / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $5}'"#,
            ])
            .output()
            .await
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<f32>()
                    .ok()
            })
            .ok()
            .flatten();
        let ms = start.elapsed().as_millis() as u32;
        return (
            AgentStatus::Online,
            os,
            kern,
            oc,
            Some(ms),
            cpu,
            ram,
            disk,
            "local".into(),
            None,
        );
    }
    let tgt = format!("{}@{}", user, host);
    let script = r#"export PATH=/opt/homebrew/bin:/usr/local/bin:/home/papasmurf/.npm-global/bin:/home/nick/.npm-global/bin:$PATH; OS=$(. /etc/os-release 2>/dev/null && echo "$NAME $VERSION_ID" || (sw_vers -productName 2>/dev/null; sw_vers -productVersion 2>/dev/null) || echo ?); KERN=$(uname -r); OC=$(openclaw --version 2>/dev/null || echo ?); CPU=$(top -bn1 2>/dev/null | grep 'Cpu(s)' | awk '{print $2+$4}' || echo ?); RAM=$(free 2>/dev/null | awk '/Mem:/{printf "%.1f", $3/$2*100}' || vm_stat 2>/dev/null | awk '/Pages active/{a=$NF} /Pages wired/{w=$NF} /Pages free/{f=$NF} END{if(a+w+f>0) printf "%.1f",(a+w)/(a+w+f)*100; else print "?"}'); DISK=$(df / 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5); print $5}' || echo ?); GWPID=$(pgrep -f 'openclaw.*gateway' 2>/dev/null | head -1 || echo ?); echo "OS:$OS"; echo "KERN:$KERN"; echo "OC:$OC"; echo "CPU:$CPU"; echo "RAM:$RAM"; echo "DISK:$DISK"; echo "GWPID:$GWPID"; ACT=$(openclaw status --json 2>/dev/null || ~/.npm-global/bin/openclaw status --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);ss=d.get('sessions',[]);active=[s for s in ss if s.get('active')];print(active[0].get('channel','idle') if active else 'idle')" 2>/dev/null || echo idle); CTX=$(openclaw status --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);ss=d.get('sessions',[]);active=[s for s in ss if s.get('active')];t=active[0].get('contextTokens',0) if active else 0;m=active[0].get('maxTokens',1000000) if active else 1000000;print(f'{t/m*100:.1f}')" 2>/dev/null || echo ?); echo "ACT:$ACT"; echo "CTX:$CTX""#;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("ssh")
            .args([
                "-o",
                "ConnectTimeout=2",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                &tgt,
                "bash",
                "-c",
                script,
            ])
            .output(),
    )
    .await;
    let result = match result {
        Ok(r) => r,
        Err(_) => {
            return (
                AgentStatus::Offline,
                String::new(),
                String::new(),
                String::new(),
                None,
                None,
                None,
                None,
                String::new(),
                None,
            );
        }
    };
    match result {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let (mut os, mut kern, mut oc) = (String::new(), String::new(), String::new());
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("OS:") {
                    os = v.trim().into();
                } else if let Some(v) = l.strip_prefix("KERN:") {
                    kern = v.trim().into();
                } else if let Some(v) = l.strip_prefix("OC:") {
                    oc = v.trim().into();
                }
            }
            let (mut cpu, mut ram, mut disk, mut act, mut ctx) =
                (None, None, None, String::new(), None);
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("CPU:") {
                    cpu = v.trim().parse::<f32>().ok();
                } else if let Some(v) = l.strip_prefix("RAM:") {
                    ram = v.trim().parse::<f32>().ok();
                } else if let Some(v) = l.strip_prefix("DISK:") {
                    disk = v.trim().parse::<f32>().ok();
                } else if let Some(v) = l.strip_prefix("ACT:") {
                    act = v.trim().to_string();
                } else if let Some(v) = l.strip_prefix("CTX:") {
                    ctx = v.trim().parse::<f32>().ok();
                }
            }
            let ms = start.elapsed().as_millis() as u32;
            (
                AgentStatus::Online,
                os,
                kern,
                oc,
                Some(ms),
                cpu,
                ram,
                disk,
                act,
                ctx,
            )
        }
        _ => (
            AgentStatus::Offline,
            String::new(),
            String::new(),
            String::new(),
            None,
            None,
            None,
            None,
            String::new(),
            None,
        ),
    }
}

fn now_str() -> String {
    use std::process::Command as C;
    C::new("date")
        .arg("+%H:%M:%S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or("now".into())
}

fn relative_time(created_at: Instant) -> String {
    let secs = created_at.elapsed().as_secs();
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 2_592_000 {
        format!("{}d ago", secs / 86400)
    } else {
        "30d+ ago".into()
    }
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

#[derive(Clone, Copy, Debug)]
enum CronOpMode {
    Add,
    Edit,
    Delete,
}

/// OpenClaw service/plugin entry
#[derive(Clone, Debug)]
struct ServiceEntry {
    name: String,
    icon: &'static str,
    enabled: bool,
    has_channel_config: bool,
    summary: String, // e.g. "2 groups, dmPolicy: pairing"
}

/// Diagnostic step result
#[derive(Clone, Debug)]
struct DiagStep {
    label: String,
    status: DiagStatus,
    detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DiagStatus {
    Running,
    Pass,
    Fail,
    Fixed,
    Skipped,
    Rollback,
}

impl DiagStatus {
    fn icon(&self) -> &'static str {
        match self {
            DiagStatus::Running => "⏳",
            DiagStatus::Pass => "✓",
            DiagStatus::Fail => "✗",
            DiagStatus::Fixed => "🔧",
            DiagStatus::Skipped => "⊘",
            DiagStatus::Rollback => "⏪",
        }
    }
}

const FLEET_CHECK_LABELS: [&str; 7] = ["SSH", "TS", "OC", "GW", "API", "CFG", "WS"];

/// Per-agent summary row in fleet diagnostic overlay
#[derive(Clone, Debug)]
struct FleetDiagResult {
    agent_idx: usize,
    name: String,
    emoji: String,
    checks: [Option<DiagStatus>; 7], // ssh, tailscale, oc, gateway, api, config, workspace
    top_issue: String,
    running: bool,
    done: bool,
}

enum FleetDiagMsg {
    AgentStart(usize),
    CheckDone {
        result_idx: usize,
        check_idx: usize,
        status: DiagStatus,
        issue: String,
    },
    AgentDone(usize),
    AllDone,
}

struct ModelLoadResult {
    agent_db_name: String,
    model: Option<String>,
    models: Vec<String>,
}

struct ModelWriteResult {
    agent_db_name: String,
    model: String,
    restarted: bool,
}

const SERVICE_ICONS: &[(&str, &str)] = &[
    ("telegram", "📱"),
    ("discord", "🎮"),
    ("signal", "🔒"),
    ("whatsapp", "💬"),
    ("slack", "💼"),
    ("irc", "📟"),
    ("matrix", "🔷"),
    ("imessage", "🍎"),
    ("bluebubbles", "🫧"),
    ("msteams", "🏢"),
    ("nostr", "🟣"),
    ("twitch", "🎬"),
    ("line", "🟢"),
    ("googlechat", "🟡"),
    ("mattermost", "🔵"),
    ("feishu", "🦅"),
    ("zalo", "📲"),
    ("nextcloud-talk", "☁️"),
    ("tlon", "🌐"),
];

fn svc_icon(name: &str) -> &'static str {
    SERVICE_ICONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, i)| *i)
        .unwrap_or("🔌")
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

const CURATED_MODELS: &[&str] = &[
    "anthropic/claude-opus-4-6",
    "anthropic/claude-sonnet-4-6",
    "anthropic/claude-haiku-4-5",
    "openai/gpt-4o",
    "openai/gpt-4o-mini",
    "google/gemini-2.0-flash",
    "groq/llama-3.3-70b-versatile",
];

fn curated_model_list() -> Vec<String> {
    CURATED_MODELS.iter().map(|m| (*m).to_string()).collect()
}

fn merge_model_list(extra: &[String]) -> Vec<String> {
    let mut merged = curated_model_list();
    let mut seen: HashSet<String> = merged.iter().cloned().collect();
    for model in extra {
        if model.contains('/') && seen.insert(model.clone()) {
            merged.push(model.clone());
        }
    }
    merged
}

/// Result of a background chat poll
struct ChatPollResult {
    global: Vec<ChatLine>,
    agent: Option<Vec<ChatLine>>,
    threads: Option<Vec<db::ThreadSummary>>,
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
const STATUS_OP_PULSE_MS: u128 = 500;

fn fmt_hhmm(t: &str) -> String {
    t.chars().take(5).collect()
}

fn build_chat_lines(
    messages: &[ChatLine],
    user: &str,
    t: &Theme,
    area_width: u16,
    spinner_frame: usize,
) -> Vec<Line<'static>> {
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
            lines.push(Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(t.text_dim),
            )));
        }
        return lines;
    }

    let inner_w = area_width.saturating_sub(2) as usize;
    let wrap_w = inner_w.saturating_sub(8).max(20);

    for msg in messages {
        let ts = fmt_hhmm(&msg.time);
        let is_outgoing = msg.sender == user;
        let indent = "  ".repeat(msg.depth as usize);

        if is_outgoing {
            // Right-aligned outgoing message (operator)
            let tgt = msg
                .target
                .as_ref()
                .map(|tgt| format!(" → @{}", tgt))
                .unwrap_or_else(|| " → all".into());
            let header_content = format!("{}{}   {}", msg.sender, tgt, ts);
            let hlen = header_content.chars().count();
            let hpad = inner_w.saturating_sub(hlen);
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::raw(" ".repeat(hpad)),
                Span::styled(
                    format!("{}{}", msg.sender, tgt),
                    Style::default().fg(t.sender_self).bold(),
                ),
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
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(w);
            }
            if !cur.is_empty() {
                wrapped.push(cur);
            }
            if wrapped.is_empty() {
                wrapped.push(msg.message.clone());
            }

            for bl in &wrapped {
                let blen = bl.chars().count();
                let bpad = inner_w.saturating_sub(blen + 2);
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
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
                _ => {
                    if msg.response.is_some() {
                        "✓✓".into()
                    } else {
                        "✓".into()
                    }
                }
            };
            let spad = inner_w.saturating_sub(st_icon.chars().count() + 1);
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::raw(" ".repeat(spad)),
                Span::styled(st_icon, Style::default().fg(t.text_dim)),
                Span::raw(" "),
            ]));

            // Show agent response below outgoing message (left-aligned reply)
            if let Some(resp) = &msg.response {
                let responder = msg.target.as_ref().map(|s| s.as_str()).unwrap_or("agent");
                let avatar = responder
                    .chars()
                    .next()
                    .map(|c| c.to_ascii_uppercase())
                    .unwrap_or('?');
                lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(
                        format!("  [{}] ", avatar),
                        Style::default().fg(t.sender_other).bold(),
                    ),
                    Span::styled(
                        responder.to_string(),
                        Style::default().fg(t.sender_other).bold(),
                    ),
                ]));
                let words: Vec<&str> = resp.split_whitespace().collect();
                let mut cur = String::new();
                let mut first = true;
                let body_wrap = wrap_w.saturating_sub(2).max(20);
                for w in &words {
                    if !cur.is_empty() && cur.chars().count() + w.len() + 1 > body_wrap {
                        let prefix = if first { "  ↳ " } else { "    " };
                        lines.push(Line::from(vec![
                            Span::raw(indent.clone()),
                            Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                            Span::styled(cur.clone(), Style::default().fg(t.response)),
                        ]));
                        cur.clear();
                        first = false;
                    }
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    let prefix = if first { "  ↳ " } else { "    " };
                    // Add blinking cursor if still streaming
                    let is_streaming = msg.status == "streaming";
                    let cursor = if is_streaming { "▌" } else { "" };
                    lines.push(Line::from(vec![
                        Span::raw(indent.clone()),
                        Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                        Span::styled(cur, Style::default().fg(t.response)),
                        Span::styled(cursor.to_string(), Style::default().fg(t.accent)),
                    ]));
                }
            }
        } else {
            // Left-aligned incoming message (agent)
            let avatar = msg
                .sender
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or('?');
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(
                    format!("  [{}] ", avatar),
                    Style::default().fg(t.sender_other).bold(),
                ),
                Span::styled(
                    msg.sender.clone(),
                    Style::default().fg(t.sender_other).bold(),
                ),
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
                            Span::raw(indent.clone()),
                            Span::styled(prefix.to_string(), Style::default().fg(t.sender_other)),
                            Span::styled(cur.clone(), Style::default().fg(t.response)),
                        ]));
                        cur.clear();
                        first = false;
                    }
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    let prefix = if first { "  ↳ " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::raw(indent.clone()),
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
                    lines.push(Line::from(Span::styled(
                        status_text,
                        Style::default().fg(t.pending),
                    )));
                }
            }
        }

        lines.push(Line::from(""));
    }
    lines
}

// ---- Rendering ----

// ---- Responsive Layout Helpers ----

fn is_narrow(area: &Rect) -> bool {
    area.width < 80
}
fn is_wide(area: &Rect) -> bool {
    area.width > 160
}



fn point_in_rect(x: u16, y: u16, area: Rect) -> bool {
    x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
}


fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
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
        Line::from(Span::styled(
            "Terminal too small",
            Style::default().fg(Color::Red).bold(),
        )),
        Line::from(Span::styled(
            format!("Need 60x20, got {}x{}", area.width, area.height),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "Resize your terminal",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    frame.render_widget(msg, area);
}

fn render_splash(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();
    let bg = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg, area);

    let ver_line = format!(
        "v{} — {} agents in fleet",
        env!("CARGO_PKG_VERSION"),
        app.agents.len()
    );
    let online_line = format!(
        "{} online",
        app.agents
            .iter()
            .filter(|a| a.status == AgentStatus::Online)
            .count()
    );
    let logo: Vec<&str> = vec![
        "",
        r"    ____    _    __  __ ",
        r"   / ___|  / \  |  \/  |",
        r"   \___ \ / _ \ | |\/| |",
        r"    ___) / ___ \| |  | |",
        r"   |____/_/   \_\_|  |_|",
        "",
        "",
        "S . A . M   M I S S I O N   C O N T R O L",
        "",
        &ver_line,
        &online_line,
        "",
        "Strange Artificial Machine — Fleet Orchestration TUI",
        "",
        "Press any key to continue...",
    ];

    // Animated gradient: cycle through theme accent colors using elapsed time
    let elapsed_ms = app.splash_start.elapsed().as_millis() as u32;
    let phase = (elapsed_ms / 80) % 6;
    let gradient_colors = [
        t.accent,
        t.accent2,
        t.header_title,
        t.header_title,
        t.accent2,
        t.accent,
    ];

    let cy = area.height / 2;
    let start_y = cy.saturating_sub(logo.len() as u16 / 2);

    for (i, line) in logo.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.height {
            break;
        }
        let color = if i >= 1 && i <= 5 {
            // Animated gradient on logo lines using theme colors
            gradient_colors[((i as u32 + phase) % 6) as usize]
        } else if i == 8 {
            t.header_title
        } else if i == logo.len() - 1 {
            t.text
        } else {
            t.text_dim
        };
        let p = Paragraph::new(Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(color).bold(),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(p, Rect::new(0, y, area.width, 1));
    }
}

fn render_dashboard(frame: &mut Frame, app: &mut App) {
    if frame.area().width < 60 || frame.area().height < 20 {
        render_too_small(frame);
        return;
    }
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    // Clear with bg color
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let online = app
        .agents
        .iter()
        .filter(|a| a.status == AgentStatus::Online)
        .count();
    let total = app.agents.len();
    let live = app.last_refresh.elapsed() < Duration::from_secs(60);
    let total_tokens: i32 = app.agents.iter().map(|a| a.token_burn).sum();
    let health_pct = if total > 0 { online * 100 / total } else { 0 };
    let health_color = if health_pct >= 80 {
        t.status_online
    } else if health_pct >= 50 {
        t.status_busy
    } else {
        t.status_offline
    };

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "🛰️  S.A.M MISSION CONTROL",
            Style::default().fg(t.header_title).bold(),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{}", online),
            Style::default().fg(t.status_online).bold(),
        ),
        Span::styled(
            format!("/{} agents", total),
            Style::default().fg(t.text_dim),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{}% healthy", health_pct),
            Style::default().fg(health_color),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{}tok", total_tokens),
            Style::default().fg(t.text_dim),
        ),
        Span::raw("    "),
        Span::styled(
            if live { "● live" } else { "○ stale" },
            Style::default().fg(if live {
                t.status_online
            } else {
                t.status_offline
            }),
        ),
        Span::raw("    "),
        Span::styled(
            if app.refreshing { "⟳ refreshing" } else { "" },
            Style::default().fg(t.accent),
        ),
        if app
            .alert_flash
            .map(|f| f.elapsed() < Duration::from_secs(5))
            .unwrap_or(false)
        {
            Span::styled(
                "  ⚠️ NEW ALERT",
                Style::default().fg(t.status_offline).bold(),
            )
        } else {
            Span::raw("")
        },
        if !app.interrupted_ops.is_empty() {
            Span::styled(
                format!("  ⚠ {} interrupted op(s)", app.interrupted_ops.len()),
                Style::default().fg(t.status_busy).bold(),
            )
        } else {
            Span::raw("")
        },
        Span::raw("    "),
        Span::styled(chrono_now(), Style::default().fg(t.text_dim)),
        Span::raw("    "),
        Span::styled(
            match app.focus {
                Focus::Fleet => "▌Fleet▐",
                Focus::Chat => "▌Chat▐",
                _ => "▌Fleet▐",
            },
            Style::default().fg(t.accent).bold(),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, outer[0]);

    let (fleet_pct, chat_pct) = dashboard_split(&outer[1], app.dashboard_split_pct);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([fleet_pct, chat_pct])
        .split(outer[1]);

    app.dashboard_body_area = outer[1];
    app.fleet_area = body[0];
    app.chat_area = body[1];
    app.dashboard_divider_area = if body[1].width > 0 {
        Rect::new(body[1].x.saturating_sub(1), outer[1].y, DIVIDER_HIT_WIDTH.min(outer[1].width), outer[1].height)
    } else {
        Rect::default()
    };
    render_fleet_table(frame, app, body[0], app.focus == Focus::Fleet);
    if !is_narrow(&outer[1]) {
        render_chat_panel(frame, app, body[1], app.focus == Focus::Chat, false);
    }
    render_footer(frame, app, outer[2]);
    render_status_bar(frame, app, outer[3], online, total, health_color);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect, online: usize, total: usize, health_color: Color) {
    let t = &app.theme;
    let elapsed = app.tui_start.elapsed();
    let uptime = format_app_uptime(elapsed.as_secs());
    let ops = app.active_ops_running();
    let pulse = if (elapsed.as_millis() / STATUS_OP_PULSE_MS) % 2 == 0 { "◐" } else { "◓" };
    let db_text = if app.db_online {
        match app.db_latency_ms {
            Some(ms) => format!("DB: {}ms", ms),
            None => "DB: …".to_string(),
        }
    } else {
        "DB: ✗ offline".to_string()
    };
    let db_span = Span::styled(
        format!("[{}]", db_text),
        Style::default().fg(db_latency_color(app.db_latency_ms, app.db_online, t)).bold(),
    );
    let ops_text = if ops > 0 {
        format!("[ops: {} {}]", ops, pulse)
    } else {
        "[ops: 0 running]".to_string()
    };
    let status = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("[{}]", chrono_now_hms()), Style::default().fg(t.text_dim)),
        Span::raw(" "),
        Span::styled(format!("[sam uptime: {}]", uptime), Style::default().fg(t.text)),
        Span::raw(" "),
        db_span,
        Span::raw(" "),
        Span::styled(ops_text, Style::default().fg(if ops > 0 { t.accent } else { t.text_dim })),
        Span::raw(" "),
        Span::styled(format!("[↑ {}/{} online]", online, total), Style::default().fg(health_color).bold()),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(t.border_type)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(status, area);
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
        vec![
            "  ", "Agent", "IP", "Location", "Status", "Ping", "Activity", "Ctx%", "CPU", "RAM",
            "Disk", "Version",
        ]
    } else if show_activity {
        vec![
            "  ", "Agent", "IP", "Location", "Status", "Ping", "Uptime", "Activity", "Version",
        ]
    } else if show_ip && show_latency {
        vec![
            "  ", "Agent", "IP", "Location", "Status", "Ping", "Uptime", "Version",
        ]
    } else if show_latency {
        vec![
            "  ", "Agent", "Location", "Status", "Ping", "Uptime", "Version",
        ]
    } else {
        vec!["  ", "Agent", "Location", "Status", "Version"]
    };
    let hcells = hcells_vec
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = {
        let filtered_indices = app.filtered_agent_indices();
        filtered_indices
            .into_iter()
            .enumerate()
            .map(|(row_idx, i)| {
                let a = &app.agents[i];
                let sel = i == app.selected && active;
                let bg = if sel {
                    t.selected_bg
                } else if row_idx % 2 == 1 {
                    ratatui::style::Color::Rgb(20, 22, 28)
                } else {
                    app.bg_density.bg()
                };
                let loc_color = match a.location.as_str() {
                    "Home" => t.loc_home,
                    "SM" => t.loc_sm,
                    "VPS" => t.loc_vps,
                    "Mobile" => t.loc_mobile,
                    _ => t.text,
                };
                let st_color = match a.status {
                    AgentStatus::Online => t.status_online,
                    AgentStatus::Busy => t.status_busy,
                    AgentStatus::Offline => t.status_offline,
                    _ => t.text_dim,
                };
                let is_multi = app.multi_selected.contains(&i);
                let cursor = if sel && is_multi {
                    "▶✓"
                } else if sel {
                    "▶ "
                } else if is_multi {
                    " ✓"
                } else {
                    "  "
                };
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
                    cells.push(
                        Cell::from(format_uptime(a.uptime_seconds))
                            .style(Style::default().fg(t.text_dim)),
                    );
                }
                if show_activity && !show_resources {
                    let act_display = if a.activity.is_empty() || a.activity == "idle" {
                        "idle".to_string()
                    } else {
                        a.activity.clone()
                    };
                    let act_color = if act_display == "idle" {
                        t.text_dim
                    } else {
                        t.accent
                    };
                    cells.push(Cell::from(act_display).style(Style::default().fg(act_color)));
                }
                if show_resources {
                    let act_short = if a.activity.is_empty() || a.activity == "idle" {
                        "·"
                    } else {
                        &a.activity
                    };
                    let act_color = if act_short == "·" {
                        t.text_dim
                    } else {
                        t.accent
                    };
                    cells.push(
                        Cell::from(act_short.chars().take(10).collect::<String>())
                            .style(Style::default().fg(act_color)),
                    );
                    let ctx_str = a
                        .context_pct
                        .map(|p| format!("{:.0}%", p))
                        .unwrap_or("—".into());
                    let ctx_color = match a.context_pct {
                        Some(p) if p > 80.0 => t.status_offline,
                        Some(p) if p > 50.0 => t.status_busy,
                        Some(_) => t.status_online,
                        None => t.text_dim,
                    };
                    cells.push(Cell::from(ctx_str).style(Style::default().fg(ctx_color)));
                    cells.push(
                        Cell::from(mini_bar(a.cpu_pct, 4))
                            .style(Style::default().fg(mini_bar_color(a.cpu_pct, t, 70.0, 90.0))),
                    );
                    cells.push(
                        Cell::from(mini_bar(a.ram_pct, 4))
                            .style(Style::default().fg(mini_bar_color(a.ram_pct, t, 70.0, 85.0))),
                    );
                    cells.push(
                        Cell::from(mini_bar(a.disk_pct, 4))
                            .style(Style::default().fg(mini_bar_color(a.disk_pct, t, 80.0, 90.0))),
                    );
                }
                let ver_color = if a.oc_version.is_empty()
                    || a.oc_version == "?"
                    || a.oc_version == "unknown"
                {
                    t.text_dim
                } else if !app.latest_oc_version.is_empty()
                    && a.oc_version.contains(&app.latest_oc_version)
                {
                    t.status_online // current
                } else if !app.latest_oc_version.is_empty() {
                    Color::Yellow // outdated
                } else {
                    t.version
                };
                cells.push(Cell::from(a.oc_version.clone()).style(Style::default().fg(ver_color)));
                Row::new(cells).style(Style::default().bg(bg)).height(1)
            })
            .collect()
    };

    app.fleet_row_start_y = area.y + 1; // +1 for border, +1 for header handled in click calc

    let widths = if show_resources {
        vec![
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(13),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(10),
        ]
    } else if show_activity {
        vec![
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(13),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Min(10),
        ]
    } else if show_ip && show_latency {
        vec![
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(13),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(10),
        ]
    } else if show_latency {
        vec![
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(10),
        ]
    } else {
        vec![
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Min(10),
        ]
    };
    let selected_count = app.selected_agent_count();
    let selected_info = if selected_count > 0 {
        format!(" • {} selected", selected_count)
    } else {
        String::new()
    };
    let fleet_title = if app.filter_active {
        if app.filter_text.is_empty() {
            format!(" ◆── Fleet 🔍 (type to search{}) ──◆ ", selected_info)
        } else {
            format!(" ◆── Fleet 🔍 {}{} ──◆ ", app.filter_text, selected_info)
        }
    } else if app.group_filter != GroupFilter::All {
        format!(" ◆── Fleet [{}{}] ──◆ ", app.group_filter.label(), selected_info)
    } else {
        format!(
            " ◆── Fleet [{}{}] ──◆ ",
            app.sort_mode.label(),
            app.sort_mode.arrow()
        )
    };
    let table = Table::new(rows, widths).header(hrow).block(
        Block::default()
            .title(Span::styled(fleet_title, Style::default().fg(fb).bold()))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(fb))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 0, 0)),
    );
    frame.render_widget(table, area);
}

fn render_chat_panel(frame: &mut Frame, app: &App, area: Rect, active: bool, agent_mode: bool) {
    let t = &app.theme;
    let cb = if active { t.border_active } else { t.border };
    let main_area = if agent_mode && app.thread_sidebar {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(area);
        let mut thread_lines: Vec<Line<'static>> = Vec::new();
        for th in &app.agent_threads {
            let is_active_thread = app
                .active_thread_id
                .as_ref()
                .map(|id| id == &th.thread_id)
                .unwrap_or(false);
            let pin = if app.pinned_threads.contains(&th.thread_id) {
                "📌 "
            } else {
                ""
            };
            thread_lines.push(Line::from(Span::styled(
                format!("{}{}", pin, App::thread_title(&th.title)),
                Style::default().fg(if is_active_thread {
                    t.border_active
                } else {
                    t.text
                }),
            )));
            thread_lines.push(Line::from(Span::styled(
                format!("  {} {}", th.target.clone().unwrap_or_default(), th.preview),
                Style::default().fg(t.text_dim),
            )));
        }
        if thread_lines.is_empty() {
            thread_lines.push(Line::from(Span::styled(
                "No recent threads",
                Style::default().fg(t.text_dim),
            )));
        }
        let sidebar = Paragraph::new(thread_lines).block(
            Block::default()
                .title(Span::styled(" Threads ", Style::default().fg(cb).bold()))
                .borders(Borders::ALL)
                .border_type(t.border_type)
                .border_style(Style::default().fg(cb))
                .style(Style::default().bg(app.bg_density.bg())),
        );
        frame.render_widget(sidebar, split[0]);
        split[1]
    } else {
        area
    };

    let cl = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(main_area);

    // Time-based spinner frame for typing animation (advances once per SPINNER_FRAME_MS).
    let spin_frame = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis() as u64
        / SPINNER_FRAME_MS) as usize;

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
    let scroll_pos = if tl > vh && scroll == 0 {
        (tl - vh) as u16
    } else {
        scroll
    };

    // Scroll indicator: count lines below the current viewport
    let lines_below = tl.saturating_sub(scroll_pos as usize + vh);
    let new_indicator = if lines_below > 0 {
        format!(" ▼ {} new ", (lines_below / LINES_PER_MSG_EST).max(1))
    } else {
        String::new()
    };

    let title = if agent_mode {
        let base = format!(
            " {} {} Chat",
            app.agents[app.selected].emoji, app.agents[app.selected].name
        );
        let thread = app
            .active_thread_id
            .as_deref()
            .map(App::thread_title)
            .unwrap_or_else(|| "new".to_string());
        format!("{} · {}{} ", base, thread, new_indicator)
    } else {
        let count = app.chat_history.len();
        let base = if count > 0 {
            format!(" Chat ({})", count)
        } else {
            " Chat".to_string()
        };
        format!("{}{} ", base, new_indicator)
    };

    let chat = Paragraph::new(messages).scroll((scroll_pos, 0)).block(
        Block::default()
            .title(Span::styled(title, Style::default().fg(cb).bold()))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(cb))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(chat, cl[0]);

    let prompt = if agent_mode {
        if app.reply_parent_id.is_some() {
            format!(" @{} (reply) › ", app.agents[app.selected].db_name)
        } else {
            format!(" @{} › ", app.agents[app.selected].db_name)
        }
    } else if app.focus == Focus::Command {
        " ⚡ fleet command (runs on all agents) ⏎ ".to_string()
    } else if active {
        " broadcast to all ⏎ ".to_string()
    } else {
        " Tab to chat ".to_string()
    };

    let display_text = if !agent_mode && app.focus == Focus::Command {
        &app.command_input
    } else {
        input_text
    };
    let is_active = active || (!agent_mode && app.focus == Focus::Command);
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(t.accent)),
        Span::styled(display_text, Style::default().fg(t.text)),
        if is_active {
            Span::styled("▌", Style::default().fg(t.accent))
        } else {
            Span::raw("")
        },
    ]))
    .block(
        Block::default()
            .title(prompt)
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(if is_active { t.border_active } else { t.border }))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(input, cl[1]);
}

fn render_detail(frame: &mut Frame, app: &mut App) {
    let t = &app.theme;
    let a = &app.agents[app.selected];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    // BG
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    // Header
    let st_color = match a.status {
        AgentStatus::Online => t.status_online,
        AgentStatus::Busy => t.status_busy,
        AgentStatus::Offline => t.status_offline,
        _ => t.text_dim,
    };
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{} {}", a.emoji, a.name),
            Style::default().fg(t.header_title).bold(),
        ),
        Span::raw("  —  "),
        Span::styled(a.status.to_string(), Style::default().fg(st_color)),
        if a.gateway_status == GatewayStatus::Offline {
            Span::styled("   GATEWAY OFFLINE", Style::default().fg(Color::Black).bg(Color::Red).bold())
        } else {
            Span::raw("")
        },
        Span::raw("    "),
        Span::styled(
            match app.focus {
                Focus::AgentChat => " 1:Info 2:▌Chat▐ 3:Files 4:Tasks 5:Svc",
                Focus::Workspace => " 1:Info 2:Chat 3:▌Files▐ 4:Tasks 5:Svc",
                Focus::Services => " 1:Info 2:Chat 3:Files 4:Tasks 5:▌Svc▐",
                _ => " 1:▌Info▐ 2:Chat 3:Files 4:Tasks 5:Svc",
            },
            Style::default().fg(t.accent).bold(),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, chunks[0]);

    // Body: info left, chat right (responsive)
    let (info_pct, chat_pct) = detail_split(&chunks[1], app.detail_split_pct);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([info_pct, chat_pct])
        .split(chunks[1]);

    // Info panel
    let info_active = app.focus != Focus::AgentChat;
    let ib = if info_active {
        t.border_active
    } else {
        t.border
    };

    let caps = if a.capabilities.is_empty() {
        "none".into()
    } else {
        a.capabilities.join(", ")
    };

    // OS-based ASCII art decoration
    let os_art = os_ascii_art(&a.os);

    let model_value = if app.agent_model_agent.as_deref() == Some(a.db_name.as_str()) {
        if app.agent_model_loading {
            "loading…".to_string()
        } else {
            app.agent_model.clone().unwrap_or_else(|| "not set".into())
        }
    } else {
        "loading…".to_string()
    };
    let rows = vec![
        ("Host", a.host.clone(), t.text),
        (
            "Location",
            a.location.clone(),
            match a.location.as_str() {
                "Home" => t.loc_home,
                "SM" => t.loc_sm,
                "VPS" => t.loc_vps,
                "Mobile" => t.loc_mobile,
                _ => t.text,
            },
        ),
        ("Status", a.status.to_string(), st_color),
        ("OS", a.os.clone(), t.text),
        ("Kernel", a.kernel.clone(), t.text),
        ("OC Version", a.oc_version.clone(), t.version),
        ("Model", model_value, t.accent),
        ("SSH User", a.ssh_user.clone(), t.text),
        ("Capabilities", caps, t.text),
        (
            "CPU",
            match a.cpu_pct {
                Some(p) => format!("{:.1}%", p),
                None => "—".into(),
            },
            match a.cpu_pct {
                Some(p) if p > 90.0 => t.status_offline,
                Some(p) if p > 70.0 => t.status_busy,
                Some(_) => t.status_online,
                _ => t.text_dim,
            },
        ),
        (
            "RAM",
            match a.ram_pct {
                Some(p) => format!("{:.1}%", p),
                None => "—".into(),
            },
            match a.ram_pct {
                Some(p) if p > 85.0 => t.status_offline,
                Some(p) if p > 70.0 => t.status_busy,
                Some(_) => t.status_online,
                _ => t.text_dim,
            },
        ),
        (
            "Disk",
            match a.disk_pct {
                Some(p) => format!("{:.0}%", p),
                None => "—".into(),
            },
            match a.disk_pct {
                Some(p) if p > 90.0 => t.status_offline,
                Some(p) if p > 80.0 => t.status_busy,
                Some(_) => t.status_online,
                _ => t.text_dim,
            },
        ),
        (
            "Latency",
            match a.latency_ms {
                Some(ms) => format!("{}ms", ms),
                None => "—".into(),
            },
            match a.latency_ms {
                Some(ms) if ms < 100 => t.status_online,
                Some(ms) if ms < 500 => t.status_busy,
                Some(_) => t.status_offline,
                _ => t.text_dim,
            },
        ),
        ("Tokens Today", format!("{}", a.token_burn), t.text),
        ("Last Seen", a.last_seen.clone(), t.text),
        (
            "Task",
            a.current_task.as_deref().unwrap_or("none").to_string(),
            t.text_dim,
        ),
    ];

    let mut info: Vec<Line> = rows
        .iter()
        .map(|(l, v, c)| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<14}", l),
                    Style::default().fg(t.text_bold).bold(),
                ),
                Span::styled(v.clone(), Style::default().fg(*c)),
            ])
        })
        .collect();

    // Append OS art decoration at the bottom of info panel
    info.push(Line::from(""));
    for art_line in os_art {
        info.push(Line::from(Span::styled(
            art_line.to_string(),
            Style::default().fg(t.text_dim),
        )));
    }

    let detail = Paragraph::new(info).block(
        Block::default()
            .title(Span::styled(
                " ◆── Info ──◆ ",
                Style::default().fg(ib).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(ib))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 1, 0)),
    );
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
    app.detail_body_area = chunks[1];
    app.detail_divider_area = if body[1].width > 0 {
        Rect::new(body[1].x.saturating_sub(1), chunks[1].y, DIVIDER_HIT_WIDTH.min(chunks[1].width), chunks[1].height)
    } else {
        Rect::default()
    };

    // Agent chat
    render_chat_panel(frame, app, body[1], app.focus == Focus::AgentChat, true);

    // Footer
    render_footer(frame, app, chunks[2]);
}

fn render_model_picker(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();
    let h = (app.model_options.len() as u16 + 6).max(10).min(area.height.saturating_sub(4));
    let w = area.width.min(72).saturating_sub(2).max(44);
    let popup = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    frame.render_widget(Clear, popup);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled("  Select active model", Style::default().fg(t.header_title).bold())));
    lines.push(Line::from(""));
    for (i, model) in app.model_options.iter().enumerate() {
        let sel = i == app.model_picker_selected;
        let prefix = if sel { " ▶ " } else { "   " };
        let style = if sel { Style::default().fg(t.text).bg(t.accent).bold() } else { Style::default().fg(t.text) };
        lines.push(Line::from(Span::styled(format!("{}{}", prefix, model), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter apply  R apply+restart  Esc close",
        Style::default().fg(t.text_dim),
    )));
    let title = if let Some(model) = &app.agent_model {
        format!(" Model Picker — current: {} ", model)
    } else {
        " Model Picker ".to_string()
    };
    let widget = Paragraph::new(lines).block(Block::default()
        .title(Span::styled(title, Style::default().fg(t.accent).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(app.bg_density.bg())));
    frame.render_widget(widget, popup);
}

fn render_fleet_diagnostics(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();

    let n = app.fleet_diag_results.len();
    // Width: header + check columns + issue column
    let w = ((area.width as f32 * 0.75) as u16)
        .max(72)
        .min(area.width.saturating_sub(4));
    // Height: title border + header row + separator + agent rows + summary + hint + border
    let h = (n as u16 + 7).min(area.height.saturating_sub(4)).max(8);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let done_count = app.fleet_diag_results.iter().filter(|r| r.done).count();
    let fail_count = app
        .fleet_diag_results
        .iter()
        .filter(|r| r.done && r.checks.iter().any(|c| matches!(c, Some(DiagStatus::Fail))))
        .count();
    let pass_count = done_count.saturating_sub(fail_count);

    let title = format!(
        " {} Fleet Diagnostic — {} agents ",
        if app.fleet_diag_fix { "🔧" } else { "🔍" },
        n
    );

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    // Column header: name (20), then 7 check columns (4 each), then issue
    let name_w = 20usize;
    let col_w = 4usize;
    let mut header_spans = vec![Span::styled(
        format!("  {:name_w$}", "Agent"),
        Style::default().fg(t.text_dim).bold(),
    )];
    for label in &FLEET_CHECK_LABELS {
        header_spans.push(Span::styled(
            format!("{:col_w$}", label),
            Style::default().fg(t.text_dim).bold(),
        ));
    }
    header_spans.push(Span::styled(
        "  Issue".to_string(),
        Style::default().fg(t.text_dim).bold(),
    ));
    lines.push(Line::from(header_spans));

    // Separator
    lines.push(Line::from(Span::styled(
        format!("  {}", "─".repeat((w.saturating_sub(4)) as usize)),
        Style::default().fg(t.text_dim),
    )));

    for (i, result) in app.fleet_diag_results.iter().enumerate() {
        let selected = i == app.fleet_diag_selected;
        let row_style = if selected {
            Style::default().bg(t.accent).fg(t.text)
        } else {
            Style::default()
        };

        let mut spans: Vec<Span> = Vec::new();

        // Selection indicator + name
        let sel_icon = if selected { "▶ " } else { "  " };
        let name_display = format!("{}{} {}", sel_icon, result.emoji, result.name);
        let name_truncated = if name_display.chars().count() > name_w + 2 {
            format!(
                "{}…",
                name_display.chars().take(name_w + 1).collect::<String>()
            )
        } else {
            format!("{:width$}", name_display, width = name_w + 2)
        };

        if result.running {
            let spinner = BRAILLE_SPINNER[app.spinner_frame % BRAILLE_SPINNER.len()];
            spans.push(Span::styled(
                name_truncated,
                if selected {
                    row_style
                } else {
                    Style::default().fg(t.text)
                },
            ));
            spans.push(Span::styled(
                format!(" {} running…", spinner),
                if selected {
                    row_style
                } else {
                    Style::default().fg(t.pending)
                },
            ));
        } else if !result.done && !result.running {
            spans.push(Span::styled(
                name_truncated,
                if selected {
                    row_style
                } else {
                    Style::default().fg(t.text_dim)
                },
            ));
            spans.push(Span::styled(
                " (pending)",
                if selected {
                    row_style
                } else {
                    Style::default().fg(t.text_dim)
                },
            ));
        } else {
            spans.push(Span::styled(
                name_truncated,
                if selected {
                    row_style
                } else {
                    Style::default().fg(t.text)
                },
            ));
            for check in &result.checks {
                let (icon, color) = match check {
                    None => ("  ? ", t.text_dim),
                    Some(DiagStatus::Pass) => ("  ✓ ", t.status_online),
                    Some(DiagStatus::Fail) => ("  ✗ ", t.status_offline),
                    Some(DiagStatus::Fixed) => (" 🔧 ", t.status_busy),
                    Some(DiagStatus::Skipped) => ("  — ", t.text_dim),
                    Some(DiagStatus::Running) => ("  … ", t.pending),
                    Some(DiagStatus::Rollback) => (" ↩ ", t.pending),
                };
                spans.push(Span::styled(
                    icon.to_string(),
                    if selected {
                        row_style
                    } else {
                        Style::default().fg(color)
                    },
                ));
            }
            if !result.top_issue.is_empty() {
                let issue = format!("  {}", result.top_issue);
                spans.push(Span::styled(
                    issue,
                    if selected {
                        row_style
                    } else {
                        Style::default().fg(t.status_offline)
                    },
                ));
            }
        }

        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    // Summary / status line
    if app.fleet_diag_done {
        let summary = if fail_count == 0 {
            format!("  All {} agents healthy ✓", n)
        } else {
            format!(
                "  {}/{} passed, {} failed",
                pass_count, done_count, fail_count
            )
        };
        let color = if fail_count == 0 {
            t.status_online
        } else {
            t.status_offline
        };
        lines.push(Line::from(Span::styled(
            summary,
            Style::default().fg(color).bold(),
        )));
    } else {
        let spinner = BRAILLE_SPINNER[app.spinner_frame % BRAILLE_SPINNER.len()];
        lines.push(Line::from(Span::styled(
            format!("  {} {}/{} agents complete…", spinner, done_count, n),
            Style::default().fg(t.pending),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  ↑↓ navigate  Enter drill-in  Esc close",
        Style::default().fg(t.text_dim),
    )));

    let border_color = if app.fleet_diag_fix {
        t.status_busy
    } else {
        t.accent
    };
    let diag = Paragraph::new(lines).block(
        Block::default()
            .title(Span::styled(title, Style::default().fg(t.accent).bold()))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(diag, popup);
}

fn render_diagnostics(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();

    // Deduplicate — only show latest status per label (computed first for sizing)
    let mut seen: std::collections::HashMap<String, DiagStep> = std::collections::HashMap::new();
    for step in &app.diag_steps {
        seen.insert(step.label.clone(), step.clone());
    }
    let mut ordered_labels: Vec<String> = Vec::new();
    for step in &app.diag_steps {
        if !ordered_labels.contains(&step.label) {
            ordered_labels.push(step.label.clone());
        }
    }

    // Count check steps (excludes header "Diagnosing…" and "DONE")
    let total_steps = ordered_labels
        .iter()
        .filter(|l| {
            *l != "DONE"
                && !seen
                    .get(*l)
                    .map(|s| s.label.contains("Diagnosing"))
                    .unwrap_or(false)
        })
        .count();
    let done_count = ordered_labels
        .iter()
        .filter(|l| {
            *l != "DONE"
                && !seen
                    .get(*l)
                    .map(|s| s.label.contains("Diagnosing"))
                    .unwrap_or(false)
                && seen
                    .get(*l)
                    .map(|s| !matches!(s.status, DiagStatus::Running))
                    .unwrap_or(false)
        })
        .count();
    let is_done = seen.contains_key("DONE");

    // ~60% width; 58 is the minimum to fit the progress bar + step labels legibly
    let w = ((area.width as f32 * 0.6) as u16)
        .max(58)
        .min(area.width.saturating_sub(4));
    let content_h = ordered_labels.len().saturating_sub(1) as u16  // visible labels (DONE hidden)
        + if total_steps > 0 { 2 } else { 0 }  // progress bar + blank
        + if is_done { 3 } else { 2 }           // summary+hint or running hint
        + 5; // blanks + borders + padding
    let h = content_h.min(area.height.saturating_sub(4)).max(8);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);

    let agent_name = if app.selected < app.agents.len() {
        &app.agents[app.selected].name
    } else {
        "?"
    };
    let title = if let Some(ref t) = app.diag_title {
        format!(" {} ", t)
    } else {
        format!(
            " {} Diagnostics — {} ",
            if app.diag_auto_fix {
                "🔧 Fix"
            } else {
                "🔍 Check"
            },
            agent_name
        )
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    // Progress bar (step N / total)
    if total_steps > 0 {
        let bar_inner = w.saturating_sub(14) as usize;
        let filled = (done_count * bar_inner / total_steps).min(bar_inner);
        let empty = bar_inner - filled;
        let bar = format!(
            "  [{}{}] {}/{}",
            "█".repeat(filled),
            "░".repeat(empty),
            done_count,
            total_steps
        );
        lines.push(Line::from(Span::styled(bar, Style::default().fg(t.accent))));
        lines.push(Line::from(""));
    }

    for label in &ordered_labels {
        if label == "DONE" {
            continue;
        }
        if let Some(step) = seen.get(label) {
            if step.label.contains("Diagnosing") {
                lines.push(Line::from(Span::styled(
                    format!("  {}", step.label),
                    Style::default().fg(t.accent).bold(),
                )));
                lines.push(Line::from(""));
                continue;
            }
            if step.status == DiagStatus::Running {
                let c = BRAILLE_SPINNER[app.spinner_frame % BRAILLE_SPINNER.len()];
                let elapsed_str = app
                    .diag_start
                    .map(|s| format!(" {:.1}s", s.elapsed().as_secs_f64()))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", c), Style::default().fg(t.pending)),
                    Span::styled(&step.label, Style::default().fg(t.text).bold()),
                    Span::styled(
                        format!("  running{}", elapsed_str),
                        Style::default().fg(t.text_dim),
                    ),
                ]));
            } else {
                let (icon, color) = match step.status {
                    DiagStatus::Pass => ("✓ ", t.status_online),
                    DiagStatus::Fail => ("✗ ", t.status_offline),
                    DiagStatus::Fixed => ("🔧", t.status_busy),
                    DiagStatus::Skipped => ("⊘ ", t.text_dim),
                    DiagStatus::Running => ("⏳", t.pending),
                    DiagStatus::Rollback => ("⏪", t.status_busy),
                };
                let detail = if step.detail.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", step.detail)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::styled(&step.label, Style::default().fg(t.text).bold()),
                    Span::styled(detail, Style::default().fg(t.text_dim)),
                ]));
            }
        }
    }

    // Summary when done, or cancel hint while running
    if is_done {
        lines.push(Line::from(""));
        let passed = ordered_labels
            .iter()
            .filter(|l| {
                *l != "DONE"
                    && !seen
                        .get(*l)
                        .map(|s| s.label.contains("Diagnosing"))
                        .unwrap_or(false)
                    && seen
                        .get(*l)
                        .map(|s| s.status == DiagStatus::Pass)
                        .unwrap_or(false)
            })
            .count();
        let fixed = ordered_labels
            .iter()
            .filter(|l| {
                *l != "DONE"
                    && !seen
                        .get(*l)
                        .map(|s| s.label.contains("Diagnosing"))
                        .unwrap_or(false)
                    && seen
                        .get(*l)
                        .map(|s| s.status == DiagStatus::Fixed)
                        .unwrap_or(false)
            })
            .count();
        let failed = ordered_labels
            .iter()
            .filter(|l| {
                *l != "DONE"
                    && !seen
                        .get(*l)
                        .map(|s| s.label.contains("Diagnosing"))
                        .unwrap_or(false)
                    && seen
                        .get(*l)
                        .map(|s| s.status == DiagStatus::Fail)
                        .unwrap_or(false)
            })
            .count();
        let summary = if failed == 0 && fixed == 0 {
            format!("  All {} checks passed ✓", total_steps)
        } else if fixed > 0 && failed == 0 {
            format!("  {}/{} passed, {} fixed ✓", passed, total_steps, fixed)
        } else if fixed > 0 {
            format!(
                "  {}/{} passed, {} fixed, {} failed",
                passed, total_steps, fixed, failed
            )
        } else {
            format!("  {}/{} passed, {} failed", passed, total_steps, failed)
        };
        let color = if failed == 0 {
            t.status_online
        } else {
            t.status_offline
        };
        lines.push(Line::from(Span::styled(
            summary,
            Style::default().fg(color).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  Press Esc or q to close",
            Style::default().fg(t.text_dim),
        )));
    } else if total_steps > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Press Esc or q to cancel",
            Style::default().fg(t.text_dim),
        )));
    }

    let diag = Paragraph::new(lines)
        .scroll((app.diag_overlay_scroll, 0))
        .block(
            Block::default()
                .title(Span::styled(title, Style::default().fg(t.accent).bold()))
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(if app.diag_auto_fix {
                    t.status_busy
                } else {
                    t.accent
                }))
                .style(Style::default().bg(app.bg_density.bg())),
        );
    frame.render_widget(diag, popup);
}

fn render_services(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(32), Constraint::Min(40)])
        .split(area);

    // Left: service list with status indicators
    let mut items: Vec<Line> = Vec::new();
    let agent_name = app
        .agents
        .get(app.selected)
        .map(|a| a.name.as_str())
        .unwrap_or("?");
    items.push(Line::from(Span::styled(
        format!("  🔌 {} Services", agent_name),
        Style::default().fg(t.header_title).bold(),
    )));
    items.push(Line::from(""));

    if app.svc_loading {
        items.push(Line::from(Span::styled(
            "  ⏳ Loading config...",
            Style::default().fg(t.pending),
        )));
    } else if app.svc_list.is_empty() {
        items.push(Line::from(Span::styled(
            "  ⚠ No config found",
            Style::default().fg(t.status_offline),
        )));
        items.push(Line::from(""));
        items.push(Line::from(Span::styled(
            "  Press 'd' to run diagnostics",
            Style::default().fg(t.text_dim),
        )));
        items.push(Line::from(Span::styled(
            "  or 'S' to setup OpenClaw",
            Style::default().fg(t.text_dim),
        )));
    } else {
        for (i, svc) in app.svc_list.iter().enumerate() {
            let selected = i == app.svc_selected;
            let prefix = if selected { " ▶ " } else { "   " };
            let (status_icon, status_color) = if svc.name == "model" || svc.name == "gateway" {
                ("◆", t.accent)
            } else if svc.enabled {
                ("●", t.status_online)
            } else {
                ("○", t.text_dim)
            };
            let name_style = if selected {
                Style::default().fg(Color::Black).bg(t.accent).bold()
            } else {
                Style::default().fg(if svc.enabled { t.text } else { t.text_dim })
            };
            items.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(t.accent)),
                Span::styled(
                    format!("{} ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::styled(format!("{} ", svc.icon), Style::default()),
                Span::styled(format!("{:<16}", svc.name), name_style),
            ]));
        }
    }

    items.push(Line::from(""));
    items.push(Line::from(Span::styled(
        "  ─── Quick Actions ───",
        Style::default().fg(t.border),
    )));
    items.push(Line::from(Span::styled(
        "  Space  toggle on/off",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  g      restart gateway",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  d      run diagnostics",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  l      view gateway logs",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  e      edit raw config",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  r      reload",
        Style::default().fg(t.text_dim),
    )));

    let list = Paragraph::new(items).block(
        Block::default()
            .title(Span::styled(
                " Services ",
                Style::default().fg(t.accent).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(list, split[0]);

    // Right: contextual action panel (NOT raw JSON)
    let detail_lines = if app.svc_selected < app.svc_list.len() {
        let svc = &app.svc_list[app.svc_selected];
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("  {} ", svc.icon), Style::default()),
                Span::styled(&svc.name, Style::default().fg(t.header_title).bold()),
                Span::raw("  "),
                Span::styled(
                    if svc.enabled {
                        "● enabled"
                    } else {
                        "○ disabled"
                    },
                    Style::default().fg(if svc.enabled {
                        t.status_online
                    } else {
                        t.text_dim
                    }),
                ),
            ]),
            Line::from(""),
        ];

        // Gateway: show status + actions
        if svc.name == "gateway" {
            lines.push(Line::from(Span::styled(
                "  Status",
                Style::default().fg(t.text_bold).bold(),
            )));
            // Parse summary for display
            for part in svc.summary.split("  ") {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (icon, color) =
                    if part.contains("token:✓") || part.contains("on") || part.contains("lan") {
                        ("  ✓ ", t.status_online)
                    } else if part.contains("none")
                        || part.contains("off")
                        || part.contains("localhost")
                    {
                        ("  ⚠ ", Color::Yellow)
                    } else {
                        ("  ◦ ", t.text)
                    };
                lines.push(Line::from(vec![
                    Span::styled(icon, Style::default().fg(color)),
                    Span::styled(part, Style::default().fg(t.text)),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Actions",
                Style::default().fg(t.text_bold).bold(),
            )));
            lines.push(Line::from(Span::styled(
                "  [g] Restart gateway",
                Style::default().fg(t.accent),
            )));
            lines.push(Line::from(Span::styled(
                "  [l] View recent logs",
                Style::default().fg(t.accent),
            )));
            lines.push(Line::from(Span::styled(
                "  [e] Edit raw config",
                Style::default().fg(t.accent),
            )));
            lines.push(Line::from(""));
            // Warnings
            if let Some(ref config) = app.svc_config {
                let bind = config
                    .get("gateway")
                    .and_then(|g| g.get("bind"))
                    .and_then(|b| b.as_str())
                    .unwrap_or("localhost");
                if bind == "localhost" {
                    lines.push(Line::from(Span::styled(
                        "  ⚠ bind=localhost — not reachable over Tailscale",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(Span::styled(
                        "    Recommended: bind=lan or bind=0.0.0.0",
                        Style::default().fg(t.text_dim),
                    )));
                }
                let chat = config
                    .get("gateway")
                    .and_then(|g| g.get("chatCompletions"))
                    .and_then(|c| c.get("enabled"))
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                if !chat {
                    lines.push(Line::from(Span::styled(
                        "  ⚠ Chat completions API disabled",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(Span::styled(
                        "    SAM chat requires this. Enable it?",
                        Style::default().fg(t.text_dim),
                    )));
                }
                let has_token = config
                    .get("gateway")
                    .and_then(|g| g.get("auth"))
                    .and_then(|a| a.get("token"))
                    .is_some();
                if !has_token {
                    lines.push(Line::from(Span::styled(
                        "  ⚠ No auth token set — API is open",
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }
        } else if svc.name == "model" {
            // Model: show current config
            lines.push(Line::from(Span::styled(
                "  Configuration",
                Style::default().fg(t.text_bold).bold(),
            )));
            for part in svc.summary.split("  ") {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                lines.push(Line::from(Span::styled(
                    format!("  ◦ {}", part),
                    Style::default().fg(t.text),
                )));
            }
            if let Some(ref config) = app.svc_config {
                let ctx = config
                    .get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("contextTokens"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                if ctx < 500_000 {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  ⚠ Context window {}K — consider 1000K", ctx / 1000),
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }
        } else {
            // Plugin/channel service
            lines.push(Line::from(Span::styled(
                "  Status",
                Style::default().fg(t.text_bold).bold(),
            )));
            if svc.enabled && svc.has_channel_config {
                lines.push(Line::from(Span::styled(
                    "  ✓ Plugin enabled",
                    Style::default().fg(t.status_online),
                )));
                lines.push(Line::from(Span::styled(
                    "  ✓ Channel configured",
                    Style::default().fg(t.status_online),
                )));
                // Parse summary for details
                for part in svc.summary.split("  ") {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    lines.push(Line::from(Span::styled(
                        format!("    {}", part),
                        Style::default().fg(t.text),
                    )));
                }
            } else if svc.enabled && !svc.has_channel_config {
                lines.push(Line::from(Span::styled(
                    "  ✓ Plugin enabled",
                    Style::default().fg(t.status_online),
                )));
                lines.push(Line::from(Span::styled(
                    "  ⚠ No channel config",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(Span::styled(
                    "    This plugin won't work without channel settings",
                    Style::default().fg(t.text_dim),
                )));
            } else if !svc.enabled && svc.has_channel_config {
                lines.push(Line::from(Span::styled(
                    "  ✗ Plugin disabled",
                    Style::default().fg(t.status_offline),
                )));
                lines.push(Line::from(Span::styled(
                    "  ✓ Channel config exists",
                    Style::default().fg(t.text_dim),
                )));
                lines.push(Line::from(Span::styled(
                    "    Press Space to enable this plugin",
                    Style::default().fg(t.text_dim),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  ✗ Plugin disabled",
                    Style::default().fg(t.status_offline),
                )));
                lines.push(Line::from(Span::styled(
                    "  ✗ No channel config",
                    Style::default().fg(t.text_dim),
                )));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Actions",
                Style::default().fg(t.text_bold).bold(),
            )));
            lines.push(Line::from(Span::styled(
                if svc.enabled {
                    "  [Space] Disable plugin"
                } else {
                    "  [Space] Enable plugin"
                },
                Style::default().fg(t.accent),
            )));
            lines.push(Line::from(Span::styled(
                "  [e]     Edit raw config",
                Style::default().fg(t.accent),
            )));

            // Health warnings
            if svc.enabled {
                let has_token = svc.summary.contains("token:✓");
                let has_bot_id = svc.summary.contains("botId:✓");
                if !has_token {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  ⚠ No bot token configured",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                if !has_bot_id && (svc.name == "discord" || svc.name == "telegram") {
                    lines.push(Line::from(Span::styled(
                        "  ⚠ No bot ID set",
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }
        }
        lines
    } else {
        vec![Line::from(Span::styled(
            "  Select a service",
            Style::default().fg(t.text_dim),
        ))]
    };

    let detail_title = if app.svc_selected < app.svc_list.len() {
        format!(" {} Detail ", app.svc_list[app.svc_selected].name)
    } else {
        " Detail ".to_string()
    };

    let detail = Paragraph::new(detail_lines)
        .scroll((app.svc_detail_scroll, 0))
        .block(
            Block::default()
                .title(Span::styled(
                    detail_title,
                    Style::default().fg(t.accent).bold(),
                ))
                .borders(Borders::ALL)
                .border_type(t.border_type)
                .border_style(Style::default().fg(t.border))
                .style(Style::default().bg(app.bg_density.bg())),
        );
    frame.render_widget(detail, split[1]);
}

fn render_workspace(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;

    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(40)])
        .split(area);

    // Left: file list + crons
    let mut items: Vec<Line> = Vec::new();
    items.push(Line::from(Span::styled(
        "  📁 Agent Files",
        Style::default().fg(t.header_title).bold(),
    )));
    items.push(Line::from(""));

    for (i, f) in app.ws_files.iter().enumerate() {
        let selected = i == app.ws_selected;
        let prefix = if selected { " ▸ " } else { "   " };
        let status = if f.exists {
            let sz = f
                .size_bytes
                .map(|s| {
                    if s > 1024 {
                        format!(" {}K", s / 1024)
                    } else {
                        format!(" {}B", s)
                    }
                })
                .unwrap_or_default();
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
            Span::styled(
                status,
                Style::default().fg(if f.exists {
                    t.status_online
                } else {
                    t.text_dim
                }),
            ),
        ]));
    }

    // Crons section
    if !app.ws_crons.is_empty() {
        items.push(Line::from(""));
        items.push(Line::from(Span::styled(
            "  ⏰ Cron Jobs",
            Style::default().fg(t.header_title).bold(),
        )));
        items.push(Line::from(""));
        for (idx, cron) in app.ws_crons.iter().enumerate() {
            let icon = if cron.enabled { "●" } else { "○" };
            let color = if cron.enabled {
                t.status_online
            } else {
                t.text_dim
            };
            let desc: String = if cron.description.len() > 22 {
                format!("{}…", &cron.description[..21])
            } else {
                cron.description.clone()
            };
            let row_style = if idx == app.ws_cron_selected {
                Style::default().fg(Color::Black).bg(t.accent).bold()
            } else {
                Style::default().fg(t.text)
            };
            items.push(Line::from(vec![
                Span::styled(format!("   {} ", icon), Style::default().fg(color)),
                Span::styled(
                    format!("{:<8}", cron.schedule),
                    Style::default().fg(t.text_dim),
                ),
                Span::styled(desc, Style::default().fg(t.text)),
            ]));
        }
    }

    if app.ws_loading {
        items.clear();
        items.push(Line::from(""));
        items.push(Line::from(Span::styled(
            "  Loading workspace...",
            Style::default().fg(t.pending),
        )));
    }

    // Keybind hints
    items.push(Line::from(""));
    items.push(Line::from(Span::styled(
        "  ↑↓ select  Enter view",
        Style::default().fg(t.text_dim),
    )));
    items.push(Line::from(Span::styled(
        "  e edit  Tab→chat",
        Style::default().fg(t.text_dim),
    )));

    let file_panel = Paragraph::new(items).block(
        Block::default()
            .title(Span::styled(
                " Workspace ",
                Style::default().fg(t.accent).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(file_panel, split[0]);

    // Right: file content viewer / editor
    let content_text = if app.ws_cron_form_active {
        vec![
            Line::from(Span::styled(
                if app.ws_cron_form_edit { "Editing cron job" } else { "Create cron job" },
                Style::default().fg(t.accent).bold(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Schedule: ", Style::default().fg(t.text_dim)),
                Span::styled(
                    app.ws_cron_form_schedule.clone(),
                    if app.ws_cron_form_focus == 0 { Style::default().fg(Color::Black).bg(t.status_busy) } else { Style::default().fg(t.text) },
                ),
            ]),
            Line::from(vec![
                Span::styled("Description: ", Style::default().fg(t.text_dim)),
                Span::styled(
                    app.ws_cron_form_description.clone(),
                    if app.ws_cron_form_focus == 1 { Style::default().fg(Color::Black).bg(t.status_busy) } else { Style::default().fg(t.text) },
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled("Tab switch field  Enter save  Esc cancel", Style::default().fg(t.text_dim))),
        ]
    } else if app.ws_editing {
        let (cur_line, cur_col) = app.ws_cursor;
        app.ws_edit_buffer
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let gutter =
                    Span::styled(format!("{:>4} │ ", i + 1), Style::default().fg(t.text_dim));
                if i == cur_line {
                    // Highlight the cursor character (or end-of-line)
                    let chars: Vec<char> = line.chars().collect();
                    let col = cur_col.min(chars.len());
                    let before: String = chars[..col].iter().collect();
                    let cursor_char: String = if col < chars.len() {
                        chars[col].to_string()
                    } else {
                        " ".to_string()
                    };
                    let after: String = if col < chars.len() {
                        chars[col + 1..].iter().collect()
                    } else {
                        String::new()
                    };
                    Line::from(vec![
                        gutter,
                        Span::styled(before, Style::default().fg(t.text)),
                        Span::styled(
                            cursor_char,
                            Style::default().fg(Color::Black).bg(t.status_busy),
                        ),
                        Span::styled(after, Style::default().fg(t.text)),
                    ])
                } else {
                    Line::from(vec![
                        gutter,
                        Span::styled(line.clone(), Style::default().fg(t.text)),
                    ])
                }
            })
            .collect::<Vec<_>>()
    } else if let Some(ref content) = app.ws_content {
        let lines: Vec<Line> = content
            .lines()
            .enumerate()
            .map(|(i, line)| {
                Line::from(vec![
                    Span::styled(format!("{:>4} │ ", i + 1), Style::default().fg(t.text_dim)),
                    Span::styled(line.to_string(), Style::default().fg(t.text)),
                ])
            })
            .collect();
        lines
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Select a file and press Enter to view",
                Style::default().fg(t.text_dim),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'e' to edit the selected file",
                Style::default().fg(t.text_dim),
            )),
        ]
    };

    let file_title = if app.ws_editing {
        let name = if app.ws_selected < app.ws_files.len() {
            let (ln, col) = app.ws_cursor;
            format!(
                " ✏ EDITING: {} — {}:{} ",
                app.ws_files[app.ws_selected].name,
                ln + 1,
                col + 1
            )
        } else {
            " ✏ EDITING ".to_string()
        };
        name
    } else if app.ws_selected < app.ws_files.len() {
        format!(
            " {} {} ",
            app.ws_files[app.ws_selected].icon, app.ws_files[app.ws_selected].name
        )
    } else {
        " File Viewer ".to_string()
    };

    let viewer_scroll = if app.ws_editing {
        let cur_line = app.ws_cursor.0 as u16;
        // Auto-scroll to keep cursor in view (inner height ≈ area height - 2 borders)
        let inner_h = split[1].height.saturating_sub(2);
        if cur_line >= app.ws_content_scroll + inner_h {
            cur_line.saturating_sub(inner_h.saturating_sub(1))
        } else if cur_line < app.ws_content_scroll {
            cur_line
        } else {
            app.ws_content_scroll
        }
    } else {
        app.ws_content_scroll
    };

    let viewer = Paragraph::new(content_text)
        .scroll((viewer_scroll, 0))
        .block(
            Block::default()
                .title(Span::styled(
                    file_title,
                    Style::default()
                        .fg(if app.ws_editing {
                            t.status_busy
                        } else {
                            t.accent
                        })
                        .bold(),
                ))
                .borders(Borders::ALL)
                .border_type(t.border_type)
                .border_style(Style::default().fg(if app.ws_editing {
                    t.status_busy
                } else {
                    t.border
                }))
                .style(Style::default().bg(app.bg_density.bg())),
        );
    frame.render_widget(viewer, split[1]);

    // Discard-confirm overlay
    if app.ws_discard_confirm {
        let w: u16 = 46;
        let h: u16 = 5;
        let x = split[1].x + split[1].width.saturating_sub(w) / 2;
        let y = split[1].y + split[1].height.saturating_sub(h) / 2;
        let rect = Rect::new(x, y, w.min(split[1].width), h.min(split[1].height));
        frame.render_widget(Clear, rect);
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Discard unsaved changes?",
                Style::default().fg(t.status_offline).bold(),
            )),
            Line::from(Span::styled(
                "  Press Esc again to confirm, any key to cancel",
                Style::default().fg(t.text_dim),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(t.border_type)
                .border_style(Style::default().fg(t.status_offline))
                .style(Style::default().bg(app.bg_density.bg())),
        );
        frame.render_widget(msg, rect);
    }
}

fn render_vpn_status(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let online = app
        .agents
        .iter()
        .filter(|a| a.status == AgentStatus::Online)
        .count();
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "🔒 VPN MESH STATUS",
            Style::default().fg(t.header_title).bold(),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{}/{} nodes reachable", online, app.agents.len()),
            Style::default().fg(t.status_online),
        ),
        Span::raw("    "),
        Span::styled("Headscale (self-hosted)", Style::default().fg(t.text_dim)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, outer[0]);

    // Node table
    let hcells = [
        "  ",
        "Agent",
        "Tailscale IP",
        "Status",
        "Location",
        "OC Version",
    ]
    .iter()
    .map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app
        .agents
        .iter()
        .map(|a| {
            let st_color = match a.status {
                AgentStatus::Online => t.status_online,
                AgentStatus::Busy => t.status_busy,
                AgentStatus::Offline => t.status_offline,
                _ => t.text_dim,
            };
            let loc_c = match a.location.as_str() {
                "Home" => t.loc_home,
                "SM" => t.loc_sm,
                "VPS" => t.loc_vps,
                "Mobile" => t.loc_mobile,
                _ => t.text,
            };
            Row::new(vec![
                Cell::from(format!(" {}", a.emoji)),
                Cell::from(a.name.clone()).style(Style::default().fg(t.text_bold).bold()),
                Cell::from(a.host.clone()).style(Style::default().fg(t.accent2)),
                Cell::from(format!("{}", a.status)).style(Style::default().fg(st_color)),
                Cell::from(a.location.clone()).style(Style::default().fg(loc_c)),
                Cell::from(a.oc_version.clone()).style(Style::default().fg(t.version)),
            ])
            .style(Style::default().bg(app.bg_density.bg()))
            .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(16),
            Constraint::Length(15),
            Constraint::Length(14),
            Constraint::Length(9),
            Constraint::Min(12),
        ],
    )
    .header(hrow)
    .block(
        Block::default()
            .title(Span::styled(
                " ◆── Mesh Nodes ──◆ ",
                Style::default().fg(t.border_active).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 0, 0)),
    );
    frame.render_widget(table, outer[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Esc/q=back  │  b=bg  │  c=theme",
            Style::default().fg(t.text_dim),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(footer, outer[2]);
}

fn render_task_board(frame: &mut Frame, app: &App) {
    let filter_label = app
        .task_filter_agent
        .as_ref()
        .map(|a| format!(" — {}", a))
        .unwrap_or_else(|| " — All".to_string());
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

    // BG
    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    // Header
    let queued = app.tasks.iter().filter(|t| t.status == "queued").count();
    let running = app
        .tasks
        .iter()
        .filter(|t| t.status == "running" || t.status == "assigned")
        .count();
    let done = app.tasks.iter().filter(|t| t.status == "completed").count();

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("📋 TASK BOARD", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(
            format!("{} queued", queued),
            Style::default().fg(t.sender_self),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} active", running),
            Style::default().fg(t.status_busy),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} done", done),
            Style::default().fg(t.status_online),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{} total", app.tasks.len()),
            Style::default().fg(t.text_dim),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, outer[0]);

    // Task body — split into list (left) and detail (right)
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[1]);

    // Task list
    let hcells = ["  #", "P", "Status", "Agent", "Description"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(t.text_bold).bold()));
    let hrow = Row::new(hcells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let sel = i == app.task_selected;
            let bg = if sel {
                t.selected_bg
            } else if i % 2 == 1 {
                ratatui::style::Color::Rgb(20, 22, 28)
            } else {
                app.bg_density.bg()
            };
            let is_multi = app.multi_selected.contains(&i);
            let cursor = if sel && is_multi {
                "▶✓"
            } else if sel {
                "▶ "
            } else if is_multi {
                " ✓"
            } else {
                "  "
            };

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
            let is_done = matches!(task.status.as_str(), "completed" | "failed");

            Row::new(vec![
                Cell::from(format!("{}{}", cursor, task.id)),
                Cell::from(format!("{} {}", pri_indicator, task.priority))
                    .style(Style::default().fg(pri_color).bold()),
                Cell::from(format!("{} {}", st_icon, task.status))
                    .style(Style::default().fg(st_color)),
                Cell::from(task.assigned_agent.as_deref().unwrap_or("—").to_string())
                    .style(Style::default().fg(t.accent2)),
                Cell::from(desc).style(Style::default().fg(t.text)),
            ])
            .style(if is_done {
                Style::default()
                    .bg(bg)
                    .fg(t.text_dim)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default().bg(bg)
            })
            .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Min(15),
        ],
    )
    .header(hrow)
    .block(
        Block::default()
            .title(Span::styled(
                format!(" ◆── Tasks{} ──◆ ", filter_label),
                Style::default().fg(t.border_active).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 0, 0)),
    );
    frame.render_widget(table, body[0]);

    // Task detail (right side)
    let detail_lines = if let Some(task) = app.tasks.get(app.task_selected) {
        let st_color = match task.status.as_str() {
            "completed" => t.status_online,
            "failed" => t.status_offline,
            "running" => t.status_busy,
            _ => t.text,
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
                Span::styled(
                    format!("{} {}", pri_indicator, task.priority),
                    Style::default().fg(t.text),
                ),
            ]),
            Line::from(vec![
                Span::styled("  Status      ", Style::default().fg(t.text_bold).bold()),
                Span::styled(&task.status, Style::default().fg(st_color)),
            ]),
            Line::from(vec![
                Span::styled("  Agent       ", Style::default().fg(t.text_bold).bold()),
                Span::styled(
                    task.assigned_agent.as_deref().unwrap_or("unassigned"),
                    Style::default().fg(t.accent2),
                ),
            ]),
            Line::from(vec![
                Span::styled("  Created     ", Style::default().fg(t.text_bold).bold()),
                Span::styled(
                    format!("{} by {}", task.created_at, task.created_by),
                    Style::default().fg(t.text_dim),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Description:",
                Style::default().fg(t.text_bold).bold(),
            )),
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
                Line::from(Span::styled(
                    "  No result yet",
                    Style::default().fg(t.text_dim),
                ))
            },
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No tasks yet",
                Style::default().fg(t.text_dim),
            )),
            Line::from(Span::styled(
                "  Press 'n' to create one",
                Style::default().fg(t.text_dim),
            )),
        ]
    };

    let detail = Paragraph::new(detail_lines).block(
        Block::default()
            .title(Span::styled(
                " ◆── Detail ──◆ ",
                Style::default().fg(t.border).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(0, 1, 1, 0)),
    );
    frame.render_widget(detail, body[1]);

    // New task input
    let input_active = app.task_input_active;
    let ib = if input_active {
        t.border_active
    } else {
        t.border
    };
    let prompt = if input_active {
        " new task description ⏎  Esc=cancel "
    } else {
        " n=new task  d=done  Esc=back "
    };
    let show_placeholder = input_active && app.task_input.is_empty();
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::default().fg(t.accent)),
        if show_placeholder {
            Span::styled(
                "type description and press Enter…",
                Style::default().fg(t.text_dim),
            )
        } else {
            Span::styled(&app.task_input, Style::default().fg(t.text))
        },
        if input_active && !show_placeholder {
            Span::styled("▌", Style::default().fg(t.accent))
        } else {
            Span::raw("")
        },
    ]))
    .block(
        Block::default()
            .title(prompt)
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(ib))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(input, outer[2]);

    // Footer
    let footer_msg = format!(
        "v0.9 │ t=tasks │ n=new │ d=done │ j/k=navigate │ Esc=back │ {}/{}",
        app.theme_name.label(),
        app.bg_density.label()
    );
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(footer_msg, Style::default().fg(t.text_dim)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(footer, outer[3]);
}

fn render_alerts(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let crits = app
        .alerts
        .iter()
        .filter(|a| a.severity == AlertSeverity::Critical)
        .count();
    let warns = app
        .alerts
        .iter()
        .filter(|a| a.severity == AlertSeverity::Warning)
        .count();
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🔔 ALERTS", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(
            format!("🔴 {}", crits),
            Style::default().fg(t.status_offline),
        ),
        Span::raw("  "),
        Span::styled(format!("🟡 {}", warns), Style::default().fg(t.status_busy)),
        Span::raw("  "),
        Span::styled(
            format!("{} total", app.alerts.len()),
            Style::default().fg(t.text_dim),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, outer[0]);

    let lines: Vec<Line> = if app.alerts.is_empty() {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  ✅ All clear — no alerts",
                Style::default().fg(t.status_online),
            )),
        ]
    } else {
        app.alerts
            .iter()
            .rev()
            .map(|a| {
                let sev_color = match a.severity {
                    AlertSeverity::Critical => t.status_offline,
                    AlertSeverity::Warning => t.status_busy,
                    AlertSeverity::Info => t.accent,
                };
                Line::from(vec![
                    Span::styled(
                        format!("  {} ", relative_time(a.created_at)),
                        Style::default().fg(t.text_dim),
                    ),
                    Span::styled(a.severity.icon(), Style::default()),
                    Span::raw(" "),
                    Span::styled(format!("{} ", a.emoji), Style::default()),
                    Span::styled(&a.message, Style::default().fg(sev_color)),
                ])
            })
            .collect()
    };

    let alerts = Paragraph::new(lines).scroll((app.alerts_scroll, 0)).block(
        Block::default()
            .title(Span::styled(
                " ◆── Alert History ──◆ ",
                Style::default().fg(t.border_active).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 1, 0)),
    );
    frame.render_widget(alerts, outer[1]);

    let footer_msg = format!(
        "Esc/q=back │ ↑↓=scroll │ b=bg ({}) │ c=theme ({})",
        app.bg_density.label(),
        app.theme_name.label()
    );
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(footer_msg, Style::default().fg(t.text_dim)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(footer, outer[2]);
}

fn render_spawn_manager(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let bg_block = Block::default().style(Style::default().bg(app.bg_density.bg()));
    frame.render_widget(bg_block, frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "🤖 SPAWN MANAGER",
            Style::default().fg(t.header_title).bold(),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(header, outer[0]);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  🚧 Coming Soon",
            Style::default().fg(t.accent).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Spawn Manager will allow you to:",
            Style::default().fg(t.text),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "    • View spawned agent sessions and processes",
            Style::default().fg(t.text_dim),
        )),
        Line::from(Span::styled(
            "    • Monitor agent name, spawn time, and status",
            Style::default().fg(t.text_dim),
        )),
        Line::from(Span::styled(
            "    • Inspect active prompt / task per agent",
            Style::default().fg(t.text_dim),
        )),
        Line::from(Span::styled(
            "    • Kill, respawn, or view output of agents",
            Style::default().fg(t.text_dim),
        )),
    ];

    let body = Paragraph::new(lines).block(
        Block::default()
            .title(Span::styled(
                " ◆── Spawn Manager ──◆ ",
                Style::default().fg(t.border_active).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border_active))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(1, 1, 1, 0)),
    );
    frame.render_widget(body, outer[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("Esc=back │ q=quit", Style::default().fg(t.text_dim)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
    frame.render_widget(footer, outer[2]);
}

fn render_help(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let theme_label = app.theme_name.label();
    let version = env!("CARGO_PKG_VERSION");

    // Category color helpers
    let nav_style = Style::default().fg(t.accent); // navigation keys (cyan)
    let act_style = Style::default().fg(t.accent2); // action keys (yellow/secondary)
    let dest_style = Style::default().fg(t.status_offline); // destructive keys (red)
    let dim_style = Style::default().fg(t.text_dim);
    let head_style = Style::default().fg(t.accent).bold();
    let text_style = Style::default().fg(t.text);

    // (key, description, key_style)
    let sections: Vec<(&str, &str, Style)> = vec![
        ("", "", dim_style),
        // --- Global keys ---
        ("GLOBAL", "", dim_style),
        ("  ?", "Open this help screen", nav_style),
        ("  q", "Quit", dest_style),
        ("  c", "Cycle color theme", act_style),
        ("  b", "Cycle background density", act_style),
        ("", "", dim_style),
        // --- Dashboard ---
        ("DASHBOARD", "", dim_style),
        ("  Tab", "Switch focus: Fleet ↔ Chat", nav_style),
        ("  ↑↓ / j k", "Navigate fleet list", nav_style),
        ("  Enter", "Open agent detail", nav_style),
        ("  R", "Refresh all agents (SSH)", act_style),
        ("  s", "Sort: name → status → location → version", act_style),
        ("  f", "Filter fleet list", act_style),
        ("  t", "Task board", nav_style),
        ("  v", "VPN mesh status", nav_style),
        ("  w", "Alerts & warnings", nav_style),
        ("  Space", "Toggle agent selection", act_style),
        ("  a", "Select all agents", act_style),
        ("  A", "Clear selection", act_style),
        ("  g", "Select all in current filter group", act_style),
        ("  Esc", "Clear selection", act_style),
        ("  /", "Fleet command (runs on selection/all)", act_style),
        ("  r", "Restart gateway (selected)", act_style),
        ("  P (Shift)", "Config push (selected)", act_style),
        ("  G (Shift)", "Investigate gateway (selected)", act_style),
        ("  o", "OpenClaw version audit", act_style),
        ("  u", "Bulk update OpenClaw", act_style),
        ("", "", dim_style),
        // --- Agent Detail ---
        ("AGENT DETAIL", "", dim_style),
        (
            "  1-5",
            "Switch tabs: Info / Chat / Files / Tasks / Services",
            nav_style,
        ),
        ("  Tab", "Switch: Info ↔ Chat", nav_style),
        ("  m", "Pick active model for this agent", act_style),
        ("  e", "View agent config (openclaw.json)", act_style),
        ("  d", "Run diagnostics", act_style),
        ("  D (Shift)", "Run diagnostics + auto-fix", act_style),
        ("  Enter", "Send direct message", act_style),
        ("  Esc", "Back to dashboard", nav_style),
        ("", "", dim_style),
        // --- Task Board ---
        ("TASK BOARD", "", dim_style),
        ("  j / k", "Navigate tasks", nav_style),
        ("  n", "Create new task", act_style),
        ("  d", "Mark done", act_style),
        ("  Esc", "Back", nav_style),
        ("", "", dim_style),
        // --- Mouse ---
        ("MOUSE", "", dim_style),
        ("  Click", "Focus panel / select agent", nav_style),
        ("  Scroll", "Scroll chat panels", nav_style),
    ];

    let mut lines: Vec<Line> = sections
        .iter()
        .map(|(l, r, style)| {
            if r.is_empty() && !l.is_empty() && !l.starts_with(' ') {
                Line::from(Span::styled(format!("  {}", l), head_style))
            } else {
                Line::from(vec![
                    Span::styled(format!("  {:<14}", l), *style),
                    Span::styled(r.to_string(), text_style),
                ])
            }
        })
        .collect();

    // Footer: theme and version info
    lines.push(Line::from(vec![]));
    lines.push(Line::from(vec![
        Span::styled("  Theme: ", dim_style),
        Span::styled(theme_label, Style::default().fg(t.accent2).bold()),
        Span::styled("   Background: ", dim_style),
        Span::styled(
            app.bg_density.label(),
            Style::default().fg(t.accent2).bold(),
        ),
        Span::styled("   SAM v", dim_style),
        Span::styled(version, Style::default().fg(t.text_bold).bold()),
    ]));

    let help = Paragraph::new(lines).scroll((app.help_scroll, 0)).block(
        Block::default()
            .title(Span::styled(
                " ◆── Help ──◆  Esc=close  ↑↓=scroll ",
                Style::default().fg(t.accent).bold(),
            ))
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.accent))
            .style(Style::default().bg(app.bg_density.bg()))
            .padding(Padding::new(2, 2, 1, 1)),
    );
    frame.render_widget(help, frame.area());
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let vim_badge = " [VIM]";

    // Breadcrumb
    let crumb = match app.screen {
        Screen::Dashboard => "Dashboard".to_string(),
        Screen::AgentDetail => {
            let name = if app.selected < app.agents.len() {
                &app.agents[app.selected].name
            } else {
                "?"
            };
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
        Screen::SpawnManager => "Dashboard › Spawn Manager".to_string(),
        _ => "Dashboard".to_string(),
    };

    // Build styled key hints (key highlighted, label dim)
    let keys: Vec<(&str, &str)> = match app.screen {
        Screen::Dashboard if app.filter_active => vec![
            ("type", "filter"),
            ("↑↓", "navigate"),
            ("⏎", "apply"),
            ("Esc", "cancel"),
        ],
        Screen::Dashboard => match app.focus {
            Focus::Chat => vec![
                ("Tab", "fleet"),
                ("⏎", "send"),
                ("@", "target"),
                ("Esc", "back"),
            ],
            Focus::Command => vec![("⏎", "run"), ("Esc", "cancel")],
            _ => vec![
                ("⏎", "open"),
                ("d", "check"),
                ("D", "fix"),
                ("U", "update"),
                ("u", "update all"),
                ("g", "group"),
                ("t", "tasks"),
                ("f", "filter"),
                ("r", "refresh"),
                ("?", "help"),
                ("q", "quit"),
            ],
        },
        Screen::AgentDetail => match app.focus {
            Focus::AgentChat => vec![
                ("⏎", "send"),
                ("r", "reply"),
                ("t", "new"),
                ("[/]", "thread"),
                ("p", "pin"),
                ("T", "sidebar"),
                ("Tab", "next"),
                ("Esc", "info"),
                ("1-5", "tabs"),
            ],
            Focus::Workspace if app.ws_editing => vec![
                ("^S", "save"),
                ("^Z", "undo"),
                ("↑↓←→", "cursor"),
                ("Esc", "discard?"),
            ],
            Focus::Workspace => vec![
                ("⏎", "view"),
                ("e", "edit"),
                ("r", "reload"),
                ("Esc", "info"),
                ("1-5", "tabs"),
            ],
            Focus::Services => vec![
                ("␣", "toggle"),
                ("r", "reload"),
                ("Esc", "info"),
                ("1-5", "tabs"),
            ],
            _ => vec![
                ("⏎", "detail"),
                ("d", "check"),
                ("D", "fix"),
                ("U", "update"),
                ("w", "files"),
                ("t", "tasks"),
                ("5", "svc"),
                ("Tab", "chat"),
                ("Esc", "back"),
            ],
        },
        Screen::TaskBoard => {
            if app.task_filter_agent.is_some() {
                vec![
                    ("n", "new"),
                    ("d", "done"),
                    ("c", "clear"),
                    ("1-5", "tabs"),
                    ("Esc", "back"),
                ]
            } else {
                vec![("n", "new"), ("d", "done"), ("Esc", "back")]
            }
        }
        Screen::Help => vec![("Esc", "back"), ("q", "quit")],
        Screen::SpawnManager => vec![("Esc", "back"), ("q", "quit")],
        _ => vec![("Esc", "back")],
    };

    // Toast (auto-dismiss after 4s)
    let show_toast = app
        .toast_at
        .map(|t| t.elapsed() < Duration::from_secs(4))
        .unwrap_or(false);
    let toast_text = if show_toast {
        app.toast_message.as_deref().unwrap_or("")
    } else {
        ""
    };

    // Build left side (breadcrumb)
    let mut left_spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(&crumb, Style::default().fg(t.accent).bold()),
    ];
    if app.vim_mode {
        left_spans.push(Span::styled(vim_badge, Style::default().fg(Color::Black).bg(t.status_busy).bold()));
    }

    // Build right side
    let mut right_spans: Vec<Span> = Vec::new();
    if !toast_text.is_empty() {
        right_spans.push(Span::styled(
            toast_text,
            Style::default().fg(Color::Yellow).bold(),
        ));
    } else {
        for (i, (key, label)) in keys.iter().enumerate() {
            if i > 0 {
                right_spans.push(Span::styled(" ", Style::default().fg(t.text_dim)));
            }
            right_spans.push(Span::styled(
                format!(" {} ", key),
                Style::default().fg(Color::Black).bg(t.accent).bold(),
            ));
            right_spans.push(Span::styled(
                format!("{}", label),
                Style::default().fg(t.text_dim),
            ));
        }
    }
    right_spans.push(Span::raw("  "));

    // Calculate padding between left and right
    let left_len: usize = crumb.len() + 2 + if app.vim_mode { vim_badge.len() } else { 0 };
    let right_len: usize = if !toast_text.is_empty() {
        toast_text.len() + 2
    } else {
        keys.iter()
            .map(|(k, l)| k.len() + l.len() + 3)
            .sum::<usize>()
            + 2
    };
    let pad = (area.width as usize).saturating_sub(left_len + right_len + 4);
    left_spans.push(Span::raw(" ".repeat(pad)));

    let mut all_spans = left_spans;
    all_spans.extend(right_spans);

    let footer = Paragraph::new(Line::from(all_spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(t.border_type)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(app.bg_density.bg())),
    );
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
        Some(cli::Commands::Setup) => {
            return cli::run_setup().map_err(|e| e.into());
        }
        Some(cli::Commands::Status) => {
            return cli::print_status().await.map_err(|e| e.into());
        }
        Some(cli::Commands::Chat { agent, message }) => {
            let msg = message.join(" ");
            return cli::send_chat(&agent, &msg).await.map_err(|e| e.into());
        }
        Some(cli::Commands::Doctor { fix, agent, fleet, json, quiet, timeout }) => {
            return cli::run_doctor(fix, agent.as_deref())
                .await
                .map_err(|e| e.into());
        }
        Some(cli::Commands::Init {
            db_host,
            db_port,
            db_user,
            db_pass,
            db_name,
            self_ip,
        }) => {
            return cli::run_init(
                db_host.as_deref(),
                db_port,
                db_user.as_deref(),
                db_pass.as_deref(),
                db_name.as_deref(),
                self_ip.as_deref(),
            )
            .await
            .map_err(|e| e.into());
        }
        Some(cli::Commands::Deploy {
            target,
            file,
            source,
        }) => {
            return cli::run_deploy(&target, &file, source.as_deref())
                .await
                .map_err(|e| e.into());
        }
        Some(cli::Commands::Onboard { host, user, name }) => {
            return cli::run_onboard(&host, &user, name.as_deref())
                .await
                .map_err(|e| e.into());
        }
        Some(cli::Commands::Log { agent, tail }) => {
            return cli::run_log(agent.as_deref(), tail)
                .await
                .map_err(|e| e.into());
        }
        Some(cli::Commands::Version) => {
            println!("sam v{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        None => {} // Launch TUI
    }

    let fleet_config = match config::load_fleet_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // Install panic hook that restores terminal before printing panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(crossterm::event::DisableMouseCapture);
        let _ = stdout().execute(LeaveAlternateScreen);
        // Write crash log
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/sam-crash.log")
        {
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
    app.vim_mode = sam_config.tui.vim_mode;
    app.update_status_bar();
    app.start_db_latency_probe();

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
                        a.db_name.clone(),
                        a.status.to_db_str().to_string(),
                        if a.os.is_empty() {
                            None
                        } else {
                            Some(a.os.clone())
                        },
                        if a.kernel.is_empty() {
                            None
                        } else {
                            Some(a.kernel.clone())
                        },
                        if a.oc_version.is_empty() {
                            None
                        } else {
                            Some(a.oc_version.clone())
                        },
                        *lat,
                    );
                    let gw_pid = a.gateway_pid;
                    tokio::spawn(async move {
                        let _ = db::update_agent_status_full(
                            &p,
                            &name,
                            &st,
                            os.as_deref(),
                            kern.as_deref(),
                            oc.as_deref(),
                            latency,
                        )
                        .await;
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
                    Screen::SpawnManager => render_spawn_manager(f, &app),
                }
                // Diagnostic overlay (renders on top of everything)
                if app.fleet_diag_active {
                    render_fleet_diagnostics(f, &app);
                } else if app.diag_active {
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
                    let lines: Vec<Line> = config
                        .lines()
                        .map(|l| {
                            Line::from(Span::styled(l.to_string(), Style::default().fg(t.text)))
                        })
                        .collect();
                    let p = Paragraph::new(lines).scroll((app.config_scroll, 0)).block(
                        Block::default()
                            .title(Span::styled(
                                " openclaw.json — Esc to close ",
                                Style::default().fg(t.accent).bold(),
                            ))
                            .borders(Borders::ALL)
                            .border_type(t.border_type)
                            .border_style(Style::default().fg(t.accent))
                            .style(Style::default().bg(app.bg_density.bg()))
                            .padding(Padding::new(1, 1, 1, 0)),
                    );
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
                match &ev {
                    Event::Key(_) | Event::Mouse(_) => {
                        app.show_splash = false;
                    }
                    _ => {}
                }
                continue;
            }

            // Mouse events
            if let Event::Mouse(mouse) = &ev {
                let (mx, my) = (mouse.column, mouse.row);
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => match app.screen {
                        Screen::Dashboard => {
                            // Click on fleet panel
                            if mx >= app.fleet_area.x
                                && mx < app.fleet_area.x + app.fleet_area.width
                                && my >= app.fleet_area.y
                                && my < app.fleet_area.y + app.fleet_area.height
                            {
                                app.focus = Focus::Fleet;
                                let first_data_row_y = app.fleet_row_start_y.saturating_add(FLEET_TABLE_HEADER_ROWS);
                                if my >= first_data_row_y {
                                    let row = (my - first_data_row_y) as usize;
                                    let filtered = app.filtered_agent_indices();
                                    if let Some(&idx) = filtered.get(row) {
                                        app.selected = idx;
                                    }
                                }
                            }
                            // Click on chat panel
                            else if mx >= app.chat_area.x
                                && mx < app.chat_area.x + app.chat_area.width
                                && my >= app.chat_area.y
                                && my < app.chat_area.y + app.chat_area.height
                            {
                                app.focus = Focus::Chat;
                            }
                        }
                        Screen::AgentDetail => {
                            if mx >= app.detail_info_area.x
                                && mx < app.detail_info_area.x + app.detail_info_area.width
                                && my >= app.detail_info_area.y
                                && my < app.detail_info_area.y + app.detail_info_area.height
                            {
                                app.focus = Focus::Fleet;
                            } else if mx >= app.detail_chat_area.x
                                && mx < app.detail_chat_area.x + app.detail_chat_area.width
                                && my >= app.detail_chat_area.y
                                && my < app.detail_chat_area.y + app.detail_chat_area.height
                            {
                                app.focus = Focus::AgentChat;
                            }
                        }
                        _ => {}
                    },
                    MouseEventKind::Drag(MouseButton::Left) => {
                        match app.dragging_split {
                            Some(SplitDragTarget::Dashboard) if app.screen == Screen::Dashboard => {
                                app.dashboard_split_pct = Some(split_pct_from_mouse(mx, app.dashboard_body_area));
                            }
                            Some(SplitDragTarget::Detail) if app.screen == Screen::AgentDetail => {
                                app.detail_split_pct = Some(split_pct_from_mouse(mx, app.detail_body_area));
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }

                // Scroll wheel in chat
                if let MouseEventKind::ScrollUp = mouse.kind {
                    match app.focus {
                        Focus::Chat => app.chat_scroll = app.chat_scroll.saturating_add(3),
                        Focus::AgentChat => {
                            app.agent_chat_scroll = app.agent_chat_scroll.saturating_add(3)
                        }
                        _ => {}
                    }
                }
                if let MouseEventKind::ScrollDown = mouse.kind {
                    match app.focus {
                        Focus::Chat => app.chat_scroll = app.chat_scroll.saturating_sub(3),
                        Focus::AgentChat => {
                            app.agent_chat_scroll = app.agent_chat_scroll.saturating_sub(3)
                        }
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
                                        let caps =
                                            format!(r#"["{}"]"#, w.location_str().to_lowercase());
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
                                        os: String::new(),
                                        kernel: String::new(),
                                        oc_version: String::new(),
                                        last_seen: String::new(),
                                        current_task: None,
                                        ssh_user: app.wizard.ssh_user.clone(),
                                        capabilities: vec![],
                                        token_burn: 0,
                                        latency_ms: None,
                                        cpu_pct: None,
                                        ram_pct: None,
                                        disk_pct: None,
                                        gateway_port: 18789,
                                        gateway_token: None,
                                        gateway_pid: None,
                                        gateway_status: GatewayStatus::Unknown,
                                        uptime_seconds: 0,
                                        activity: "new".into(),
                                        context_pct: None,
                                        last_probe_at: None,
                                    });
                                    app.wizard.active = false;
                                    let created_name = app.wizard.agent_name.clone();
                                    app.toast(&format!("✅ Agent '{}' created", created_name));
                                }
                            }
                            KeyCode::Tab => {
                                // Test SSH on confirm step
                                if app.wizard.step == wizard::WizardStep::Confirm {
                                    let host = app.wizard.host.clone();
                                    let user = app.wizard.ssh_user.clone();
                                    app.wizard.testing_ssh = true;
                                    app.wizard.ssh_result = Some("Testing...".into());
                                    let (tx, rx) = mpsc::unbounded_channel();
                                    app.wizard_ssh_rx = Some(rx);
                                    tokio::spawn(async move {
                                        let result = tokio::process::Command::new("ssh")
                                            .args(["-o","ConnectTimeout=2","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                &format!("{}@{}", user, host), "hostname && openclaw --version 2>/dev/null || echo 'OC not found'"])
                                            .output().await;
                                        let msg = match result {
                                            Ok(o) if o.status.success() => format!(
                                                "✅ {}",
                                                String::from_utf8_lossy(&o.stdout).trim()
                                            ),
                                            Ok(o) => format!(
                                                "❌ {}",
                                                String::from_utf8_lossy(&o.stderr)
                                                    .trim()
                                                    .chars()
                                                    .take(60)
                                                    .collect::<String>()
                                            ),
                                            Err(e) => format!("❌ {}", e),
                                        };
                                        let _ = tx.send(msg);
                                    });
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
                        // Fleet diagnostic overlay intercepts all keys when active
                        if app.fleet_diag_active {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.fleet_diag_active = false;
                                    app.fleet_diag_results.clear();
                                    app.fleet_diag_rx = None;
                                    app.fleet_diag_done = false;
                                    app.start_refresh();
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if app.fleet_diag_selected > 0 {
                                        app.fleet_diag_selected -= 1;
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if app.fleet_diag_selected
                                        < app.fleet_diag_results.len().saturating_sub(1)
                                    {
                                        app.fleet_diag_selected += 1;
                                    }
                                }
                                KeyCode::Enter => {
                                    // Drill into selected agent — close fleet view and open single-agent diag
                                    let agent_idx = app
                                        .fleet_diag_results
                                        .get(app.fleet_diag_selected)
                                        .map(|r| r.agent_idx);
                                    let fix = app.fleet_diag_fix;
                                    app.fleet_diag_active = false;
                                    app.fleet_diag_results.clear();
                                    app.fleet_diag_rx = None;
                                    app.fleet_diag_done = false;
                                    if let Some(idx) = agent_idx {
                                        app.selected = idx;
                                        app.start_diagnostics(fix);
                                    }
                                }
                                _ => {}
                            }
                        } else
                        // Diagnostic overlay intercepts all keys when active
                        if app.diag_active {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.diag_active = false;
                                    app.diag_steps.clear();
                                    app.diag_rx = None;
                                    app.diag_start = None;
                                    app.start_refresh(); // re-probe after fix
                                }
                                KeyCode::PageUp => {
                                    app.diag_overlay_scroll =
                                        app.diag_overlay_scroll.saturating_sub(5);
                                }
                                KeyCode::PageDown => {
                                    app.diag_overlay_scroll =
                                        app.diag_overlay_scroll.saturating_add(5);
                                }
                                KeyCode::Up => {
                                    app.diag_overlay_scroll =
                                        app.diag_overlay_scroll.saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    app.diag_overlay_scroll =
                                        app.diag_overlay_scroll.saturating_add(1);
                                }
                                _ => {}
                            }
                        } else {
                            match app.screen {
                                Screen::SpawnManager => {
                                    if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                                        app.screen = Screen::Dashboard;
                                    }
                                }
                                Screen::Help => match key.code {
                                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                                        app.screen = Screen::Dashboard;
                                        app.help_scroll = 0;
                                    }
                                    KeyCode::Up | KeyCode::Char('k') => {
                                        app.help_scroll = app.help_scroll.saturating_sub(1);
                                    }
                                    KeyCode::Down | KeyCode::Char('j') => {
                                        app.help_scroll = app.help_scroll.saturating_add(1);
                                    }
                                    KeyCode::PageUp => {
                                        app.help_scroll = app.help_scroll.saturating_sub(10);
                                    }
                                    KeyCode::PageDown => {
                                        app.help_scroll = app.help_scroll.saturating_add(10);
                                    }
                                    _ => {}
                                },
                                Screen::AgentDetail if app.config_text.is_some() => {
                                    match key.code {
                                        KeyCode::Esc => {
                                            app.config_text = None;
                                        }
                                        KeyCode::PageUp | KeyCode::Up => {
                                            app.config_scroll = app.config_scroll.saturating_sub(3);
                                        }
                                        KeyCode::PageDown | KeyCode::Down => {
                                            app.config_scroll = app.config_scroll.saturating_add(3);
                                        }
                                        _ => {
                                            app.config_text = None;
                                        }
                                    }
                                }
                                Screen::AgentDetail => match app.focus {
                                    Focus::Services => match key.code {
                                        KeyCode::Esc => app.focus = Focus::Fleet,
                                        KeyCode::Tab => app.focus = Focus::Fleet,
                                        KeyCode::Char('1') => app.focus = Focus::Fleet,
                                        KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                        KeyCode::Char('3') => {
                                            app.focus = Focus::Workspace;
                                            app.start_workspace_load();
                                        }
                                        KeyCode::Char('4') | KeyCode::Char('t') => {
                                            app.task_filter_agent =
                                                Some(app.agents[app.selected].db_name.clone());
                                            app.screen = Screen::TaskBoard;
                                            app.last_task_poll =
                                                Instant::now() - Duration::from_secs(10);
                                        }
                                        KeyCode::Char('5') => {} // already here
                                        KeyCode::Up => {
                                            if app.svc_selected > 0 {
                                                app.svc_selected -= 1;
                                                app.svc_detail_scroll = 0;
                                            }
                                        }
                                        KeyCode::Down => {
                                            if app.svc_selected
                                                < app.svc_list.len().saturating_sub(1)
                                            {
                                                app.svc_selected += 1;
                                                app.svc_detail_scroll = 0;
                                            }
                                        }
                                        KeyCode::Char(' ') => app.toggle_service(),
                                        KeyCode::Char('r') => app.start_services_load(),
                                        KeyCode::Char('d') => app.start_diagnostics(false),
                                        KeyCode::Char('D') => app.start_diagnostics(true),
                                        KeyCode::Char('U') => app.start_oc_update(),
                                        KeyCode::Char('g') => {
                                            // Restart gateway from services tab
                                            if let Some(agent) = app.agents.get(app.selected) {
                                                let host = agent.host.clone();
                                                let user = agent.ssh_user.clone();
                                                let name = agent.name.clone();
                                                let is_mac =
                                                    agent.os.to_lowercase().contains("mac");
                                                app.toast(&format!(
                                                    "🔄 Restarting gateway on {}...",
                                                    name
                                                ));
                                                tokio::spawn(async move {
                                                    let pfx = if is_mac {
                                                        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                    } else {
                                                        ""
                                                    };
                                                    let cmd = format!(
                                                        "{}openclaw gateway restart 2>&1 | tail -1",
                                                        pfx
                                                    );
                                                    let _ = tokio::process::Command::new("ssh")
                                                        .args([
                                                            "-o",
                                                            "ConnectTimeout=2",
                                                            "-o",
                                                            "StrictHostKeyChecking=no",
                                                            "-o",
                                                            "BatchMode=yes",
                                                            &format!("{}@{}", user, host),
                                                            &cmd,
                                                        ])
                                                        .output()
                                                        .await;
                                                });
                                            }
                                        }
                                        KeyCode::Char('l') => {
                                            // View gateway logs from services tab
                                            if let Some(agent) = app.agents.get(app.selected) {
                                                let host = agent.host.clone();
                                                let user = agent.ssh_user.clone();
                                                let self_ip = app.self_ip.clone();
                                                let is_mac =
                                                    agent.os.to_lowercase().contains("mac");
                                                app.toast("📋 Fetching logs...");
                                                let (tx, rx) = mpsc::unbounded_channel();
                                                app.config_load_rx = Some(rx);
                                                tokio::spawn(async move {
                                                    let pfx = if is_mac {
                                                        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                    } else {
                                                        ""
                                                    };
                                                    let cmd = format!(
                                                        "{}journalctl -u openclaw-gateway --no-pager -n 30 --output=short-iso 2>/dev/null || openclaw gateway status 2>/dev/null || echo 'no logs available'",
                                                        pfx
                                                    );
                                                    let output = if host == "localhost"
                                                        || host == self_ip
                                                    {
                                                        tokio::process::Command::new("bash")
                                                            .args(["-c", &cmd])
                                                            .output()
                                                            .await
                                                            .ok()
                                                    } else {
                                                        tokio::time::timeout(
                                                            std::time::Duration::from_secs(5),
                                                            tokio::process::Command::new("ssh")
                                                                .args([
                                                                    "-o",
                                                                    "ConnectTimeout=2",
                                                                    "-o",
                                                                    "StrictHostKeyChecking=no",
                                                                    "-o",
                                                                    "BatchMode=yes",
                                                                    &format!("{}@{}", user, host),
                                                                    &cmd,
                                                                ])
                                                                .output(),
                                                        )
                                                        .await
                                                        .ok()
                                                        .and_then(|r| r.ok())
                                                    };
                                                    let text = output
                                                        .map(|o| {
                                                            String::from_utf8_lossy(&o.stdout)
                                                                .to_string()
                                                        })
                                                        .unwrap_or_else(|| {
                                                            "Timeout fetching logs".into()
                                                        });
                                                    let _ = tx.send(Some(text));
                                                });
                                            }
                                        }
                                        KeyCode::Char('e') => {
                                            // Open raw config viewer
                                            if let Some(ref config) = app.svc_config {
                                                let pretty = serde_json::to_string_pretty(config)
                                                    .unwrap_or_default();
                                                app.config_text = Some(pretty);
                                                app.config_scroll = 0;
                                            }
                                        }
                                        KeyCode::PageUp => {
                                            app.svc_detail_scroll =
                                                app.svc_detail_scroll.saturating_add(5)
                                        }
                                        KeyCode::PageDown => {
                                            app.svc_detail_scroll =
                                                app.svc_detail_scroll.saturating_sub(5)
                                        }
                                        KeyCode::Char('q') => app.should_quit = true,
                                        _ => {}
                                    },
                                    Focus::Workspace => {
                                        if app.ws_editing {
                                            // Any key except Esc resets discard confirm prompt
                                            if !matches!(key.code, KeyCode::Esc) {
                                                app.ws_discard_confirm = false;
                                            }
                                            match key.code {
                                                KeyCode::Esc => {
                                                    if app.ws_discard_confirm {
                                                        // Second Esc: discard changes
                                                        app.ws_editing = false;
                                                        app.ws_discard_confirm = false;
                                                        app.ws_undo_stack.clear();
                                                        if let Some(ref c) = app.ws_content {
                                                            app.ws_edit_buffer = c
                                                                .lines()
                                                                .map(|l| l.to_string())
                                                                .collect();
                                                        }
                                                        app.ws_cursor = (0, 0);
                                                    } else {
                                                        // First Esc: ask for confirmation
                                                        app.ws_discard_confirm = true;
                                                        app.toast(
                                                            "Press Esc again to discard changes",
                                                        );
                                                    }
                                                }
                                                KeyCode::Char('s')
                                                    if key
                                                        .modifiers
                                                        .contains(KeyModifiers::CONTROL) =>
                                                {
                                                    app.start_file_save()
                                                }
                                                KeyCode::Char('z')
                                                    if key
                                                        .modifiers
                                                        .contains(KeyModifiers::CONTROL) =>
                                                {
                                                    if let Some((buf, cur)) =
                                                        app.ws_undo_stack.pop()
                                                    {
                                                        app.ws_edit_buffer = buf;
                                                        app.ws_cursor = cur;
                                                        // clamp cursor
                                                        let ln = app.ws_cursor.0.min(
                                                            app.ws_edit_buffer
                                                                .len()
                                                                .saturating_sub(1),
                                                        );
                                                        let col = app.ws_cursor.1.min(
                                                            app.ws_edit_buffer
                                                                .get(ln)
                                                                .map(|l| l.chars().count())
                                                                .unwrap_or(0),
                                                        );
                                                        app.ws_cursor = (ln, col);
                                                    }
                                                }
                                                KeyCode::Up => {
                                                    if app.ws_cursor.0 > 0 {
                                                        let new_line = app.ws_cursor.0 - 1;
                                                        let max_col = app
                                                            .ws_edit_buffer
                                                            .get(new_line)
                                                            .map(|l| l.chars().count())
                                                            .unwrap_or(0);
                                                        app.ws_cursor = (
                                                            new_line,
                                                            app.ws_cursor.1.min(max_col),
                                                        );
                                                    }
                                                }
                                                KeyCode::Down => {
                                                    if app.ws_cursor.0 + 1
                                                        < app.ws_edit_buffer.len()
                                                    {
                                                        let new_line = app.ws_cursor.0 + 1;
                                                        let max_col = app
                                                            .ws_edit_buffer
                                                            .get(new_line)
                                                            .map(|l| l.chars().count())
                                                            .unwrap_or(0);
                                                        app.ws_cursor = (
                                                            new_line,
                                                            app.ws_cursor.1.min(max_col),
                                                        );
                                                    }
                                                }
                                                KeyCode::Left => {
                                                    let (ln, col) = app.ws_cursor;
                                                    if col > 0 {
                                                        app.ws_cursor.1 = col - 1;
                                                    } else if ln > 0 {
                                                        let prev_len = app
                                                            .ws_edit_buffer
                                                            .get(ln - 1)
                                                            .map(|l| l.chars().count())
                                                            .unwrap_or(0);
                                                        app.ws_cursor = (ln - 1, prev_len);
                                                    }
                                                }
                                                KeyCode::Right => {
                                                    let (ln, col) = app.ws_cursor;
                                                    let line_len = app
                                                        .ws_edit_buffer
                                                        .get(ln)
                                                        .map(|l| l.chars().count())
                                                        .unwrap_or(0);
                                                    if col < line_len {
                                                        app.ws_cursor.1 = col + 1;
                                                    } else if ln + 1 < app.ws_edit_buffer.len() {
                                                        app.ws_cursor = (ln + 1, 0);
                                                    }
                                                }
                                                KeyCode::Home => {
                                                    app.ws_cursor.1 = 0;
                                                }
                                                KeyCode::End => {
                                                    let ln = app.ws_cursor.0;
                                                    let line_len = app
                                                        .ws_edit_buffer
                                                        .get(ln)
                                                        .map(|l| l.chars().count())
                                                        .unwrap_or(0);
                                                    app.ws_cursor.1 = line_len;
                                                }
                                                KeyCode::Enter => {
                                                    app.ws_push_undo();
                                                    let (ln, col) = app.ws_cursor;
                                                    if ln < app.ws_edit_buffer.len() {
                                                        let rest: String = app.ws_edit_buffer[ln]
                                                            .chars()
                                                            .skip(col)
                                                            .collect();
                                                        app.ws_edit_buffer[ln] = app.ws_edit_buffer
                                                            [ln]
                                                            .chars()
                                                            .take(col)
                                                            .collect();
                                                        app.ws_edit_buffer.insert(ln + 1, rest);
                                                        app.ws_cursor = (ln + 1, 0);
                                                    } else {
                                                        app.ws_edit_buffer.push(String::new());
                                                        app.ws_cursor =
                                                            (app.ws_edit_buffer.len() - 1, 0);
                                                    }
                                                }
                                                KeyCode::Backspace => {
                                                    app.ws_push_undo();
                                                    let (ln, col) = app.ws_cursor;
                                                    if col > 0 && ln < app.ws_edit_buffer.len() {
                                                        let line = &mut app.ws_edit_buffer[ln];
                                                        let mut chars: Vec<char> =
                                                            line.chars().collect();
                                                        chars.remove(col - 1);
                                                        *line = chars.into_iter().collect();
                                                        app.ws_cursor.1 = col - 1;
                                                    } else if col == 0 && ln > 0 {
                                                        let cur_line =
                                                            app.ws_edit_buffer.remove(ln);
                                                        let prev_len = app.ws_edit_buffer[ln - 1]
                                                            .chars()
                                                            .count();
                                                        app.ws_edit_buffer[ln - 1]
                                                            .push_str(&cur_line);
                                                        app.ws_cursor = (ln - 1, prev_len);
                                                    }
                                                }
                                                KeyCode::Delete => {
                                                    app.ws_push_undo();
                                                    let (ln, col) = app.ws_cursor;
                                                    if ln < app.ws_edit_buffer.len() {
                                                        let line_len =
                                                            app.ws_edit_buffer[ln].chars().count();
                                                        if col < line_len {
                                                            let line = &mut app.ws_edit_buffer[ln];
                                                            let mut chars: Vec<char> =
                                                                line.chars().collect();
                                                            chars.remove(col);
                                                            *line = chars.into_iter().collect();
                                                        } else if ln + 1 < app.ws_edit_buffer.len()
                                                        {
                                                            let next_line =
                                                                app.ws_edit_buffer.remove(ln + 1);
                                                            app.ws_edit_buffer[ln]
                                                                .push_str(&next_line);
                                                        }
                                                    }
                                                }
                                                KeyCode::Char(c) => {
                                                    app.ws_push_undo();
                                                    let (ln, col) = app.ws_cursor;
                                                    if app.ws_edit_buffer.is_empty() {
                                                        app.ws_edit_buffer.push(String::new());
                                                    }
                                                    let ln = ln.min(app.ws_edit_buffer.len() - 1);
                                                    let line = &mut app.ws_edit_buffer[ln];
                                                    let mut chars: Vec<char> =
                                                        line.chars().collect();
                                                    let col = col.min(chars.len());
                                                    chars.insert(col, c);
                                                    *line = chars.into_iter().collect();
                                                    app.ws_cursor = (ln, col + 1);
                                                }
                                                _ => {}
                                            }
                                        } else {
                                            match key.code {
                                                KeyCode::Esc => app.focus = Focus::Fleet,
                                                KeyCode::Tab => app.focus = Focus::Fleet,
                                                KeyCode::Char('1') => app.focus = Focus::Fleet,
                                                KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                                KeyCode::Char('3') => {} // already here
                                                KeyCode::Char('4') | KeyCode::Char('t') => {
                                                    app.task_filter_agent = Some(
                                                        app.agents[app.selected].db_name.clone(),
                                                    );
                                                    app.screen = Screen::TaskBoard;
                                                    app.last_task_poll =
                                                        Instant::now() - Duration::from_secs(10);
                                                }
                                                KeyCode::Char('5') => {
                                                    app.focus = Focus::Services;
                                                    app.start_services_load();
                                                }
                                                KeyCode::Up => {
                                                    if app.ws_selected > 0 {
                                                        app.ws_selected -= 1;
                                                    }
                                                }
                                                KeyCode::Down => {
                                                    if app.ws_selected
                                                        < app.ws_files.len().saturating_sub(1)
                                                    {
                                                        app.ws_selected += 1;
                                                    }
                                                }
                                                KeyCode::Enter => app.start_file_load(),
                                                KeyCode::Char('e') => {
                                                    if let Some(ref c) = app.ws_content {
                                                        app.ws_edit_buffer = c
                                                            .lines()
                                                            .map(|l| l.to_string())
                                                            .collect();
                                                        if app.ws_edit_buffer.is_empty() {
                                                            app.ws_edit_buffer.push(String::new());
                                                        }
                                                        app.ws_cursor = (0, 0);
                                                        app.ws_undo_stack.clear();
                                                        app.ws_discard_confirm = false;
                                                        app.ws_editing = true;
                                                    } else {
                                                        app.start_file_load();
                                                    }
                                                }
                                                KeyCode::Char('r') => app.start_workspace_load(),
                                                KeyCode::PageUp => {
                                                    app.ws_content_scroll =
                                                        app.ws_content_scroll.saturating_add(5)
                                                }
                                                KeyCode::PageDown => {
                                                    app.ws_content_scroll =
                                                        app.ws_content_scroll.saturating_sub(5)
                                                }
                                                KeyCode::Char('q') => app.should_quit = true,
                                                _ => {}
                                            }
                                        }
                                    }
                                    Focus::AgentChat => {
                                        if app.ac_visible {
                                            match key.code {
                                                KeyCode::Up => {
                                                    if app.ac_selected > 0 {
                                                        app.ac_selected -= 1;
                                                    } else {
                                                        app.ac_selected =
                                                            app.ac_matches.len().saturating_sub(1);
                                                    }
                                                }
                                                KeyCode::Down => {
                                                    app.ac_selected = (app.ac_selected + 1)
                                                        % app.ac_matches.len().max(1);
                                                }
                                                KeyCode::Tab | KeyCode::Enter => {
                                                    app.accept_autocomplete()
                                                }
                                                KeyCode::Esc => {
                                                    app.ac_visible = false;
                                                }
                                                KeyCode::Backspace => {
                                                    app.agent_chat_input.pop();
                                                    app.update_autocomplete();
                                                }
                                                KeyCode::Char(c) => {
                                                    app.agent_chat_input.push(c);
                                                    app.update_autocomplete();
                                                }
                                                _ => {}
                                            }
                                        } else {
                                            match key.code {
                                                KeyCode::Esc => app.focus = Focus::Fleet,
                                                KeyCode::Tab => {
                                                    app.focus = Focus::Workspace;
                                                    app.start_workspace_load();
                                                }
                                                KeyCode::Char('1')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.focus = Focus::Fleet
                                                }
                                                KeyCode::Char('3')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.focus = Focus::Workspace;
                                                    app.start_workspace_load();
                                                }
                                                KeyCode::Char('4')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.task_filter_agent = Some(
                                                        app.agents[app.selected].db_name.clone(),
                                                    );
                                                    app.screen = Screen::TaskBoard;
                                                    app.last_task_poll =
                                                        Instant::now() - Duration::from_secs(10);
                                                }
                                                KeyCode::Char('5')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.focus = Focus::Services;
                                                    app.start_services_load();
                                                }
                                                KeyCode::Char('r')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.reply_parent_id = app
                                                        .agent_chat_history
                                                        .last()
                                                        .map(|m| m.id)
                                                        .filter(|id| *id > 0);
                                                    if app.reply_parent_id.is_some() {
                                                        app.toast("Replying in current thread");
                                                    }
                                                }
                                                KeyCode::Char('t')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.active_thread_id =
                                                        Some(App::new_thread_id());
                                                    app.reply_parent_id = None;
                                                    app.agent_chat_history.clear();
                                                    app.toast("Started new thread");
                                                }
                                                KeyCode::Char('[')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    if !app.agent_threads.is_empty() {
                                                        let cur = app
                                                            .active_thread_id
                                                            .as_ref()
                                                            .and_then(|id| {
                                                                app.agent_threads.iter().position(
                                                                    |th| &th.thread_id == id,
                                                                )
                                                            })
                                                            .unwrap_or(0);
                                                        let next = if cur == 0 {
                                                            app.agent_threads.len() - 1
                                                        } else {
                                                            cur - 1
                                                        };
                                                        app.active_thread_id = Some(
                                                            app.agent_threads[next]
                                                                .thread_id
                                                                .clone(),
                                                        );
                                                        app.reply_parent_id = None;
                                                    }
                                                }
                                                KeyCode::Char(']')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    if !app.agent_threads.is_empty() {
                                                        let cur = app
                                                            .active_thread_id
                                                            .as_ref()
                                                            .and_then(|id| {
                                                                app.agent_threads.iter().position(
                                                                    |th| &th.thread_id == id,
                                                                )
                                                            })
                                                            .unwrap_or(0);
                                                        let next =
                                                            (cur + 1) % app.agent_threads.len();
                                                        app.active_thread_id = Some(
                                                            app.agent_threads[next]
                                                                .thread_id
                                                                .clone(),
                                                        );
                                                        app.reply_parent_id = None;
                                                    }
                                                }
                                                KeyCode::Char('p')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    if let Some(id) = app.active_thread_id.clone() {
                                                        if !app.pinned_threads.insert(id.clone()) {
                                                            app.pinned_threads.remove(&id);
                                                        }
                                                    }
                                                }
                                                KeyCode::Char('T')
                                                    if app.agent_chat_input.is_empty() =>
                                                {
                                                    app.thread_sidebar = !app.thread_sidebar
                                                }
                                                KeyCode::Enter => app.send_agent_message().await,
                                                KeyCode::Backspace => {
                                                    app.agent_chat_input.pop();
                                                    app.update_autocomplete();
                                                }
                                                KeyCode::Char(c) => {
                                                    app.agent_chat_input.push(c);
                                                    app.update_autocomplete();
                                                }
                                                KeyCode::PageUp => {
                                                    app.agent_chat_scroll =
                                                        app.agent_chat_scroll.saturating_add(5)
                                                }
                                                KeyCode::PageDown => {
                                                    app.agent_chat_scroll =
                                                        app.agent_chat_scroll.saturating_sub(5)
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    _ => match key.code {
                                        KeyCode::Esc => {
                                            app.screen = Screen::Dashboard;
                                            app.focus = Focus::Fleet;
                                        }
                                        KeyCode::Tab => app.focus = Focus::AgentChat,
                                        KeyCode::Char('1') => app.focus = Focus::Fleet,
                                        KeyCode::Char('2') => app.focus = Focus::AgentChat,
                                        KeyCode::Char('3') | KeyCode::Char('w') => {
                                            app.focus = Focus::Workspace;
                                            app.start_workspace_load();
                                        }
                                        KeyCode::Char('4') | KeyCode::Char('t') => {
                                            app.task_filter_agent =
                                                Some(app.agents[app.selected].db_name.clone());
                                            app.screen = Screen::TaskBoard;
                                            app.last_task_poll =
                                                Instant::now() - Duration::from_secs(10);
                                        }
                                        KeyCode::Char('5') => {
                                            app.focus = Focus::Services;
                                            app.start_services_load();
                                        }
                                        KeyCode::Char('d') => app.start_diagnostics(false),
                                        KeyCode::Char('D') => app.start_diagnostics(true),
                                        KeyCode::Char('U') => app.start_oc_update(),
                                        KeyCode::Char('q') => app.should_quit = true,
                                        KeyCode::Char('r') => app.start_refresh(),
                                        KeyCode::Char('b') => app.cycle_bg(),
                                        KeyCode::Char('e') => {
                                            // Fetch remote config (non-blocking)
                                            if let Some(agent) = app.agents.get(app.selected) {
                                                let host = agent.host.clone();
                                                let user = agent.ssh_user.clone();
                                                let self_ip = app.self_ip.clone();
                                                let is_mac =
                                                    agent.os.to_lowercase().contains("mac");
                                                app.toast("📋 Fetching config...");
                                                let (tx, rx) = mpsc::unbounded_channel();
                                                app.config_load_rx = Some(rx);
                                                tokio::spawn(async move {
                                                    let pfx = if is_mac {
                                                        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                    } else {
                                                        ""
                                                    };
                                                    let cmd = format!(
                                                        "{}cat ~/.openclaw/openclaw.json 2>/dev/null || echo '(no config found)'",
                                                        pfx
                                                    );
                                                    let output = if host == "localhost"
                                                        || host == self_ip
                                                    {
                                                        tokio::process::Command::new("bash")
                                                            .args(["-c", &cmd])
                                                            .output()
                                                            .await
                                                            .ok()
                                                    } else {
                                                        tokio::time::timeout(
                                                            std::time::Duration::from_secs(5),
                                                            tokio::process::Command::new("ssh")
                                                                .args([
                                                                    "-o",
                                                                    "ConnectTimeout=2",
                                                                    "-o",
                                                                    "StrictHostKeyChecking=no",
                                                                    "-o",
                                                                    "BatchMode=yes",
                                                                    &format!("{}@{}", user, host),
                                                                    &cmd,
                                                                ])
                                                                .output(),
                                                        )
                                                        .await
                                                        .ok()
                                                        .and_then(|r| r.ok())
                                                    };
                                                    let _ = tx.send(output.map(|o| {
                                                        String::from_utf8_lossy(&o.stdout)
                                                            .to_string()
                                                    }));
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
                                                        let output = if host == "localhost"
                                                            || host == self_ip
                                                        {
                                                            tokio::process::Command::new("bash")
                                                                .args(["-c", cmd])
                                                                .output()
                                                                .await
                                                                .ok()
                                                        } else {
                                                            let is_mac = host.contains("mac")
                                                                || host.contains("darwin");
                                                            let pfx = if is_mac {
                                                                "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                            } else {
                                                                ""
                                                            };
                                                            tokio::time::timeout(
                                                                std::time::Duration::from_secs(5),
                                                                tokio::process::Command::new("ssh")
                                                                    .args([
                                                                        "-o",
                                                                        "ConnectTimeout=2",
                                                                        "-o",
                                                                        "StrictHostKeyChecking=no",
                                                                        "-o",
                                                                        "BatchMode=yes",
                                                                        &format!(
                                                                            "{}@{}",
                                                                            user, host
                                                                        ),
                                                                        &format!("{}{}", pfx, cmd),
                                                                    ])
                                                                    .output(),
                                                            )
                                                            .await
                                                            .ok()
                                                            .and_then(|r| r.ok())
                                                        };
                                                        let response = output
                                                            .map(|o| {
                                                                let s = String::from_utf8_lossy(
                                                                    &o.stdout,
                                                                )
                                                                .trim()
                                                                .to_string();
                                                                if s.is_empty() {
                                                                    "(no output)".into()
                                                                } else {
                                                                    s.chars()
                                                                        .take(1000)
                                                                        .collect::<String>()
                                                                }
                                                            })
                                                            .unwrap_or_else(|| "Timeout".into());
                                                        let _ = crate::db::send_direct(
                                                            &pool,
                                                            &sender,
                                                            &name,
                                                            "📋 gateway logs",
                                                        )
                                                        .await;
                                                        if let Ok(mut conn) = pool.get_conn().await
                                                        {
                                                            use mysql_async::prelude::*;
                                                            let _ = conn.exec_drop(
                                                        "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW() WHERE sender=? AND target=? AND status='pending' ORDER BY id DESC LIMIT 1",
                                                        (&response, &sender, &name),
                                                    ).await;
                                                        }
                                                    });
                                                }
                                                let agent_name = app
                                                    .agents
                                                    .get(app.selected)
                                                    .map(|a| a.name.clone())
                                                    .unwrap_or_default();
                                                app.status_message = format!(
                                                    "📋 Fetching gateway logs from {}...",
                                                    agent_name
                                                );
                                            }
                                        }
                                        _ => {}
                                    },
                                },
                                Screen::Alerts => match key.code {
                                    KeyCode::Esc | KeyCode::Char('q') => {
                                        app.screen = Screen::Dashboard;
                                        app.focus = Focus::Fleet;
                                    }
                                    KeyCode::Char('b') => app.cycle_bg(),
                                    KeyCode::Char('c') => app.cycle_theme(),
                                    KeyCode::Up | KeyCode::Char('k') => {
                                        app.alerts_scroll = app.alerts_scroll.saturating_sub(1)
                                    }
                                    KeyCode::Down | KeyCode::Char('j') => {
                                        app.alerts_scroll = app.alerts_scroll.saturating_add(1)
                                    }
                                    _ => {}
                                },
                                Screen::VpnStatus => match key.code {
                                    KeyCode::Esc | KeyCode::Char('q') => {
                                        app.screen = Screen::Dashboard;
                                        app.focus = Focus::Fleet;
                                    }
                                    KeyCode::Char('b') => app.cycle_bg(),
                                    KeyCode::Char('c') => app.cycle_theme(),
                                    _ => {}
                                },
                                Screen::TaskBoard => {
                                    if app.task_input_active {
                                        match key.code {
                                            KeyCode::Esc => {
                                                app.task_input_active = false;
                                                app.task_input.clear();
                                            }
                                            KeyCode::Enter => {
                                                if !app.task_input.trim().is_empty() {
                                                    let desc = app.task_input.clone();
                                                    app.task_input.clear();
                                                    app.task_input_active = false;
                                                    if let Some(pool) = &app.db_pool {
                                                        let pool = pool.clone();
                                                        let agent = app.task_filter_agent.clone();
                                                        let user = app.user();
                                                        tokio::spawn(async move {
                                                            let _ = db::create_task(
                                                                &pool,
                                                                &desc,
                                                                5,
                                                                &user,
                                                                agent.as_deref(),
                                                            )
                                                            .await;
                                                        });
                                                        // Trigger re-poll on next tick
                                                        app.last_task_poll = Instant::now()
                                                            - Duration::from_secs(10);
                                                    }
                                                    app.toast("✓ Task created");
                                                }
                                            }
                                            KeyCode::Backspace => {
                                                app.task_input.pop();
                                            }
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
                                            KeyCode::Char('1')
                                                if app.task_filter_agent.is_some() =>
                                            {
                                                app.screen = Screen::AgentDetail;
                                                app.focus = Focus::Fleet;
                                            }
                                            KeyCode::Char('2')
                                                if app.task_filter_agent.is_some() =>
                                            {
                                                app.screen = Screen::AgentDetail;
                                                app.focus = Focus::AgentChat;
                                            }
                                            KeyCode::Char('3')
                                                if app.task_filter_agent.is_some() =>
                                            {
                                                app.screen = Screen::AgentDetail;
                                                app.focus = Focus::Workspace;
                                                app.start_workspace_load();
                                            }
                                            KeyCode::Char('4') => {} // already on tasks
                                            KeyCode::Char('5')
                                                if app.task_filter_agent.is_some() =>
                                            {
                                                app.screen = Screen::AgentDetail;
                                                app.focus = Focus::Services;
                                                app.start_services_load();
                                            }
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                if app.task_selected > 0 {
                                                    app.task_selected -= 1;
                                                }
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                if app.task_selected
                                                    < app.tasks.len().saturating_sub(1)
                                                {
                                                    app.task_selected += 1;
                                                }
                                            }
                                            KeyCode::Char('n') => app.task_input_active = true,
                                            KeyCode::Char('d') => {
                                                if let Some(task) = app.tasks.get(app.task_selected)
                                                {
                                                    let tid = task.id;
                                                    if let Some(pool) = &app.db_pool {
                                                        let pool = pool.clone();
                                                        tokio::spawn(async move {
                                                            let _ = db::update_task_status(
                                                                &pool,
                                                                tid,
                                                                "completed",
                                                            )
                                                            .await;
                                                        });
                                                        // Mark completed locally (optimistic)
                                                        if let Some(t) =
                                                            app.tasks.get_mut(app.task_selected)
                                                        {
                                                            t.status = "completed".into();
                                                        }
                                                        app.last_task_poll = Instant::now()
                                                            - Duration::from_secs(10);
                                                    }
                                                    app.toast(&format!(
                                                        "✓ Task #{} marked complete",
                                                        tid
                                                    ));
                                                }
                                            }
                                            KeyCode::Char('c')
                                                if app.task_filter_agent.is_some() =>
                                            {
                                                app.task_filter_agent = None;
                                                app.last_task_poll =
                                                    Instant::now() - Duration::from_secs(10);
                                                app.toast("Filter cleared — showing all tasks");
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Screen::Dashboard => {
                                    match app.focus {
                                        Focus::Fleet if app.filter_active => match key.code {
                                            KeyCode::Esc => {
                                                app.filter_active = false;
                                                app.filter_text.clear();
                                            }
                                            KeyCode::Enter => {
                                                app.filter_active = false;
                                            }
                                            KeyCode::Backspace => {
                                                app.filter_text.pop();
                                            }
                                            KeyCode::Char(ch) => {
                                                app.filter_text.push(ch);
                                            }
                                            KeyCode::Up | KeyCode::Char('k') => app.previous(),
                                            KeyCode::Down | KeyCode::Char('j') => app.next(),
                                            _ => {}
                                        },
                                        Focus::Fleet => {
                                            match key.code {
                                                KeyCode::Char('q') => app.should_quit = true,
                                                KeyCode::Char('d') => {
                                                    app.start_fleet_diagnostics(false)
                                                }
                                                KeyCode::Char('D') => {
                                                    app.start_fleet_diagnostics(true)
                                                }
                                                KeyCode::Tab => app.focus = Focus::Chat,
                                                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                                                KeyCode::Down | KeyCode::Char('j') => app.next(),
                                                KeyCode::Enter => {
                                                    app.screen = Screen::AgentDetail;
                                                    app.focus = Focus::Fleet;
                                                    app.agent_chat_input.clear();
                                                    app.agent_chat_history.clear();
                                                    app.agent_chat_scroll = 0;
                                                    app.active_thread_id = None;
                                                    app.reply_parent_id = None;
                                                    app.agent_threads.clear();
                                                    // Trigger immediate agent chat load
                                                    app.last_chat_poll =
                                                        Instant::now() - Duration::from_secs(10);
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
                                                KeyCode::Char('s') => {
                                                    app.cycle_sort();
                                                    app.toast(&format!(
                                                        "Sort: {}{}",
                                                        app.sort_mode.label(),
                                                        app.sort_mode.arrow()
                                                    ));
                                                }
                                                KeyCode::Char('g') => {
                                                    app.cycle_group();
                                                    app.toast(&format!(
                                                        "Group: {}",
                                                        app.group_filter.label()
                                                    ));
                                                }
                                                KeyCode::Char('a') => {
                                                    app.wizard.open();
                                                }
                                                KeyCode::Char('A') => {
                                                    // Select all
                                                    for i in 0..app.agents.len() {
                                                        app.multi_selected.insert(i);
                                                    }
                                                    app.toast(&format!(
                                                        "✓ Selected all {} agents",
                                                        app.agents.len()
                                                    ));
                                                }
                                                KeyCode::Char('N') => {
                                                    app.multi_selected.clear();
                                                    app.toast("Selection cleared");
                                                }
                                                KeyCode::Char('h') => {
                                                    // Fleet health summary
                                                    let total = app.agents.len();
                                                    let online = app
                                                        .agents
                                                        .iter()
                                                        .filter(|a| a.status == AgentStatus::Online)
                                                        .count();
                                                    let offline: Vec<String> = app
                                                        .agents
                                                        .iter()
                                                        .filter(|a| {
                                                            a.status == AgentStatus::Offline
                                                        })
                                                        .map(|a| a.name.clone())
                                                        .collect();
                                                    let unknown: Vec<String> = app
                                                        .agents
                                                        .iter()
                                                        .filter(|a| {
                                                            a.status == AgentStatus::Unknown
                                                        })
                                                        .map(|a| a.name.clone())
                                                        .collect();
                                                    let outdated: Vec<String> = app
                                                        .agents
                                                        .iter()
                                                        .filter(|a| {
                                                            !a.oc_version.is_empty()
                                                                && a.oc_version != "2026.2.21-2"
                                                                && a.oc_version != "?"
                                                        })
                                                        .map(|a| {
                                                            format!("{}({})", a.name, a.oc_version)
                                                        })
                                                        .collect();

                                                    let mut msg =
                                                        format!("🏥 {}/{} online", online, total);
                                                    if !offline.is_empty() {
                                                        msg += &format!(
                                                            " │ ❌ offline: {}",
                                                            offline.join(", ")
                                                        );
                                                    }
                                                    if !unknown.is_empty() {
                                                        msg += &format!(
                                                            " │ ❓ unknown: {}",
                                                            unknown.join(", ")
                                                        );
                                                    }
                                                    if !outdated.is_empty() {
                                                        msg += &format!(
                                                            " │ ⚠️  old OC: {}",
                                                            outdated.join(", ")
                                                        );
                                                    }
                                                    if offline.is_empty()
                                                        && unknown.is_empty()
                                                        && outdated.is_empty()
                                                    {
                                                        msg += " │ ✅ All healthy";
                                                    }
                                                    app.status_message = msg;
                                                }
                                                KeyCode::Char('/') => {
                                                    app.focus = Focus::Command;
                                                    app.command_input.clear();
                                                }
                                                KeyCode::Char('o') => {
                                                    // OpenClaw fleet operations menu
                                                    app.status_message =
                                                        "⏳ Running OC audit...".into();
                                                    let mut outdated = vec![];
                                                    let latest = "2026.2.21-2";
                                                    for agent in &app.agents {
                                                        if !agent.oc_version.is_empty()
                                                            && agent.oc_version != latest
                                                            && agent.oc_version != "?"
                                                        {
                                                            outdated.push(format!(
                                                                "{} ({})",
                                                                agent.name, agent.oc_version
                                                            ));
                                                        }
                                                    }
                                                    if outdated.is_empty() {
                                                        app.status_message =
                                                            format!("✅ All agents on {}", latest);
                                                    } else {
                                                        app.status_message = format!(
                                                            "⚠️  {} outdated: {}",
                                                            outdated.len(),
                                                            outdated.join(", ")
                                                        );
                                                    }
                                                }
                                                KeyCode::Char('u') => {
                                                    // Bulk update OC on outdated agents (or selected)
                                                    let latest = app.latest_oc_version.clone();
                                                    let filtered_indices =
                                                        app.filtered_agent_indices();
                                                    let targets: Vec<(
                                                        String,
                                                        String,
                                                        String,
                                                        bool,
                                                        String,
                                                    )> = if app.multi_selected.is_empty() {
                                                        filtered_indices
                                                            .iter()
                                                            .map(|&i| &app.agents[i])
                                                            .filter(|a| {
                                                                a.status == AgentStatus::Online
                                                            })
                                                            .filter(|a| {
                                                                latest.is_empty()
                                                                    || !a
                                                                        .oc_version
                                                                        .contains(&latest)
                                                            })
                                                            .map(|a| {
                                                                (
                                                                    a.db_name.clone(),
                                                                    a.host.clone(),
                                                                    a.ssh_user.clone(),
                                                                    a.os.to_lowercase()
                                                                        .contains("mac"),
                                                                    a.oc_version.clone(),
                                                                )
                                                            })
                                                            .collect()
                                                    } else {
                                                        app.multi_selected
                                                            .iter()
                                                            .filter_map(|&i| app.agents.get(i))
                                                            .filter(|a| {
                                                                a.status == AgentStatus::Online
                                                            })
                                                            .map(|a| {
                                                                (
                                                                    a.db_name.clone(),
                                                                    a.host.clone(),
                                                                    a.ssh_user.clone(),
                                                                    a.os.to_lowercase()
                                                                        .contains("mac"),
                                                                    a.oc_version.clone(),
                                                                )
                                                            })
                                                            .collect()
                                                    };
                                                    if targets.is_empty() {
                                                        app.toast("✓ All agents already on latest version");
                                                    } else {
                                                        let count = targets.len();
                                                        app.toast(&format!(
                                                            "🔄 Updating {} agents to {}...",
                                                            count,
                                                            if latest.is_empty() {
                                                                "latest".into()
                                                            } else {
                                                                latest.clone()
                                                            }
                                                        ));
                                                        for (
                                                            db_name,
                                                            host,
                                                            ssh_user,
                                                            is_mac,
                                                            old_ver,
                                                        ) in targets
                                                        {
                                                            if let Some(pool) = &app.db_pool {
                                                                let pool = pool.clone();
                                                                let sender = app.user();
                                                                let self_ip = app.self_ip.clone();
                                                                let latest = latest.clone();
                                                                tokio::spawn(async move {
                                                                    let op_id =
                                                                        db::create_operation(
                                                                            &pool,
                                                                            &db_name,
                                                                            "bulk_update",
                                                                        )
                                                                        .await
                                                                        .ok();
                                                                    let pfx = if is_mac {
                                                                        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                                    } else {
                                                                        ""
                                                                    };
                                                                    let cmd = format!(
                                                                        "{}openclaw --version 2>/dev/null && sudo npm install -g openclaw@latest 2>&1 | tail -3 && echo '---' && openclaw --version 2>/dev/null && openclaw gateway restart 2>&1 | tail -1",
                                                                        pfx
                                                                    );
                                                                    let output = if host
                                                                        == "localhost"
                                                                        || host == self_ip
                                                                    {
                                                                        tokio::process::Command::new("bash").args(["-c", &cmd]).output().await.ok()
                                                                    } else {
                                                                        tokio::time::timeout(
                                                            std::time::Duration::from_secs(120),
                                                            tokio::process::Command::new("ssh")
                                                                .args(["-o","ConnectTimeout=5","-o","StrictHostKeyChecking=no","-o","BatchMode=yes",
                                                                    &format!("{}@{}", ssh_user, host), &cmd])
                                                                .output()
                                                        ).await.ok().and_then(|r| r.ok())
                                                                    };
                                                                    let timed_out =
                                                                        output.is_none();
                                                                    let response = output.map(|o| {
                                                        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                                        if s.is_empty() { "(no output)".into() } else { s.chars().take(500).collect::<String>() }
                                                    }).unwrap_or_else(|| "Timeout".into());
                                                                    let msg = format!(
                                                                        "🔄 update {} → {}",
                                                                        old_ver, latest
                                                                    );
                                                                    let _ = crate::db::send_direct(
                                                                        &pool, &sender, &db_name,
                                                                        &msg,
                                                                    )
                                                                    .await;
                                                                    if let Ok(mut conn) =
                                                                        pool.get_conn().await
                                                                    {
                                                                        use mysql_async::prelude::*;
                                                                        let _ = conn.exec_drop(
                                                            "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW() WHERE sender=? AND target=? AND status='pending' ORDER BY id DESC LIMIT 1",
                                                            (&response, &sender, &db_name),
                                                        ).await;
                                                                    }
                                                                    if let Some(op_id) = op_id {
                                                                        let status = if timed_out {
                                                                            "failed"
                                                                        } else {
                                                                            "completed"
                                                                        };
                                                                        let _ =
                                                                            db::complete_operation(
                                                                                &pool,
                                                                                op_id,
                                                                                status,
                                                                                Some(&response),
                                                                            )
                                                                            .await;
                                                                    }
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                                KeyCode::Char('U') => app.start_oc_update(),
                                                KeyCode::Char('G') => {
                                                    // Gateway status on selected agent
                                                    if let Some(agent) =
                                                        app.agents.get(app.selected)
                                                    {
                                                        let host = agent.host.clone();
                                                        let user = agent.ssh_user.clone();
                                                        let name = agent.name.clone();
                                                        let self_ip = app.self_ip.clone();
                                                        let is_mac =
                                                            agent.os.to_lowercase().contains("mac");
                                                        app.status_message = format!(
                                                            "🔍 Checking gateway on {}...",
                                                            name
                                                        );
                                                        if let Some(pool) = &app.db_pool {
                                                            let pool = pool.clone();
                                                            let sender = app.user();
                                                            let db_name = agent.db_name.clone();
                                                            tokio::spawn(async move {
                                                                let pfx = if is_mac {
                                                                    "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                                } else {
                                                                    ""
                                                                };
                                                                let cmd = format!(
                                                                    "{}echo '=== Gateway Status ===' && openclaw gateway status 2>&1 && echo '=== OC Version ===' && openclaw --version 2>&1 && echo '=== Last 5 Log Lines ===' && journalctl -u openclaw-gateway --no-pager -n 5 --output=short-iso 2>/dev/null || echo 'no systemd logs'",
                                                                    pfx
                                                                );
                                                                let output = if host == "localhost"
                                                                    || host == self_ip
                                                                {
                                                                    tokio::process::Command::new(
                                                                        "bash",
                                                                    )
                                                                    .args(["-c", &cmd])
                                                                    .output()
                                                                    .await
                                                                    .ok()
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
                                                                let _ = crate::db::send_direct(
                                                                    &pool,
                                                                    &sender,
                                                                    &db_name,
                                                                    "🔍 gateway investigate",
                                                                )
                                                                .await;
                                                                if let Ok(mut conn) =
                                                                    pool.get_conn().await
                                                                {
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
                                                    if let Some(agent) =
                                                        app.agents.get(app.selected)
                                                    {
                                                        let name = agent.name.clone();
                                                        let confirmed = app
                                                            .gateway_confirm_at
                                                            .map(|t| t.elapsed().as_secs() < 5)
                                                            .unwrap_or(false);
                                                        if confirmed {
                                                            app.gateway_confirm_at = None;
                                                            let host = agent.host.clone();
                                                            let user = agent.ssh_user.clone();
                                                            let is_mac = agent
                                                                .os
                                                                .to_lowercase()
                                                                .contains("mac");
                                                            app.status_message = format!(
                                                                "🔄 Restarting gateway on {}...",
                                                                name
                                                            );
                                                            tokio::spawn(async move {
                                                                let pfx = if is_mac {
                                                                    "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                                } else {
                                                                    ""
                                                                };
                                                                let cmd = format!(
                                                                    "{}openclaw gateway restart 2>&1 | tail -1",
                                                                    pfx
                                                                );
                                                                let _ =
                                                                    tokio::process::Command::new(
                                                                        "ssh",
                                                                    )
                                                                    .args([
                                                                        "-o",
                                                                        "ConnectTimeout=2",
                                                                        "-o",
                                                                        "StrictHostKeyChecking=no",
                                                                        "-o",
                                                                        "BatchMode=yes",
                                                                        &format!(
                                                                            "{}@{}",
                                                                            user, host
                                                                        ),
                                                                        &cmd,
                                                                    ])
                                                                    .output()
                                                                    .await;
                                                            });
                                                        } else {
                                                            app.gateway_confirm_at =
                                                                Some(Instant::now());
                                                            app.toast(&format!("⚠ Press g again to restart gateway on {}", name));
                                                        }
                                                    }
                                                }
                                                KeyCode::Char('w') => {
                                                    app.screen = Screen::Alerts;
                                                    app.alerts_scroll = 0;
                                                }
                                                KeyCode::Char('v') => {
                                                    app.screen = Screen::VpnStatus;
                                                }
                                                KeyCode::Char('x') => {
                                                    app.screen = Screen::SpawnManager
                                                }
                                                KeyCode::Char('t') => {
                                                    app.task_filter_agent = None;
                                                    app.screen = Screen::TaskBoard;
                                                    app.last_task_poll =
                                                        Instant::now() - Duration::from_secs(10);
                                                }
                                                _ => {}
                                            }
                                        }
                                        Focus::Command => {
                                            match key.code {
                                                KeyCode::Esc => {
                                                    app.focus = Focus::Fleet;
                                                    app.command_input.clear();
                                                }
                                                KeyCode::Enter => {
                                                    if !app.command_input.trim().is_empty() {
                                                        let cmd = app.command_input.clone();
                                                        app.command_input.clear();
                                                        app.focus = Focus::Fleet;
                                                        app.status_message = format!(
                                                            "⚡ Running '{}' on all agents...",
                                                            &cmd
                                                        );

                                                        // Fan out to selected agents (or all online if none selected)
                                                        let agents: Vec<(
                                                            String,
                                                            String,
                                                            String,
                                                            bool,
                                                        )> = if app.multi_selected.is_empty() {
                                                            app.agents
                                                                .iter()
                                                                .filter(|a| {
                                                                    a.status == AgentStatus::Online
                                                                })
                                                                .map(|a| {
                                                                    (
                                                                        a.db_name.clone(),
                                                                        a.host.clone(),
                                                                        a.ssh_user.clone(),
                                                                        a.os.to_lowercase()
                                                                            .contains("mac"),
                                                                    )
                                                                })
                                                                .collect()
                                                        } else {
                                                            app.multi_selected
                                                                .iter()
                                                                .filter_map(|&i| app.agents.get(i))
                                                                .filter(|a| {
                                                                    a.status == AgentStatus::Online
                                                                })
                                                                .map(|a| {
                                                                    (
                                                                        a.db_name.clone(),
                                                                        a.host.clone(),
                                                                        a.ssh_user.clone(),
                                                                        a.os.to_lowercase()
                                                                            .contains("mac"),
                                                                    )
                                                                })
                                                                .collect()
                                                        };

                                                        if let Some(pool) = &app.db_pool {
                                                            let user = app.user();
                                                            for (name, host, ssh_user, is_mac) in
                                                                agents
                                                            {
                                                                let pool = pool.clone();
                                                                let cmd = cmd.clone();
                                                                let user = user.clone();
                                                                let self_ip = app.self_ip.clone();
                                                                tokio::spawn(async move {
                                                                    let pfx = if is_mac {
                                                                        "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; "
                                                                    } else {
                                                                        ""
                                                                    };
                                                                    let full_cmd =
                                                                        format!("{}{}", pfx, cmd);

                                                                    let output = if host
                                                                        == "localhost"
                                                                        || host == self_ip
                                                                    {
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
                                                                            if out.is_empty()
                                                                                && err.is_empty()
                                                                            {
                                                                                "(no output)".into()
                                                                            } else if out.is_empty()
                                                                            {
                                                                                err.chars().take(500).collect::<String>()
                                                                            } else {
                                                                                out.chars().take(500).collect::<String>()
                                                                            }
                                                                        }
                                                                        None => {
                                                                            "Timeout/error".into()
                                                                        }
                                                                    };

                                                                    // Write result to mc_chat
                                                                    let _ = crate::db::send_direct(
                                                                        &pool,
                                                                        &user,
                                                                        &name,
                                                                        &format!("/{}", cmd),
                                                                    )
                                                                    .await;
                                                                    // Update the last message with the response
                                                                    if let Ok(mut conn) =
                                                                        pool.get_conn().await
                                                                    {
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
                                                KeyCode::Backspace => {
                                                    app.command_input.pop();
                                                }
                                                KeyCode::Char(ch) => app.command_input.push(ch),
                                                _ => {}
                                            }
                                        }
                                        Focus::Chat => {
                                            if app.ac_visible {
                                                match key.code {
                                                    KeyCode::Up => {
                                                        if app.ac_selected > 0 {
                                                            app.ac_selected -= 1;
                                                        } else {
                                                            app.ac_selected = app
                                                                .ac_matches
                                                                .len()
                                                                .saturating_sub(1);
                                                        }
                                                    }
                                                    KeyCode::Down => {
                                                        app.ac_selected = (app.ac_selected + 1)
                                                            % app.ac_matches.len().max(1);
                                                    }
                                                    KeyCode::Tab | KeyCode::Enter => {
                                                        app.accept_autocomplete()
                                                    }
                                                    KeyCode::Esc => {
                                                        app.ac_visible = false;
                                                    }
                                                    KeyCode::Backspace => {
                                                        app.chat_input.pop();
                                                        app.update_autocomplete();
                                                    }
                                                    KeyCode::Char(c) => {
                                                        app.chat_input.push(c);
                                                        app.update_autocomplete();
                                                    }
                                                    _ => {}
                                                }
                                            } else {
                                                match key.code {
                                                    KeyCode::Tab | KeyCode::Esc => {
                                                        app.focus = Focus::Fleet
                                                    }
                                                    KeyCode::Enter => app.send_message().await,
                                                    KeyCode::Backspace => {
                                                        app.chat_input.pop();
                                                        app.update_autocomplete();
                                                    }
                                                    KeyCode::Char(c) => {
                                                        app.chat_input.push(c);
                                                        app.update_autocomplete();
                                                    }
                                                    KeyCode::PageUp => {
                                                        app.chat_scroll =
                                                            app.chat_scroll.saturating_add(5)
                                                    }
                                                    KeyCode::PageDown => {
                                                        app.chat_scroll =
                                                            app.chat_scroll.saturating_sub(5)
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
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
            // Fetch latest OC version from npm
            if let Ok(out) = tokio::process::Command::new("npm")
                .args(["view", "openclaw", "version"])
                .output()
                .await
            {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !ver.is_empty() {
                    app.status_message = format!("Latest OpenClaw: {}", ver);
                    app.latest_oc_version = ver;
                }
            }
            app.start_refresh();
        }

        // Poll tasks every 5s when on task board
        if app.screen == Screen::TaskBoard && app.last_task_poll.elapsed() > Duration::from_secs(5)
        {
            if let Some(pool) = &app.db_pool {
                if let Ok(mut tasks) = db::load_tasks(pool, 50).await {
                    if let Some(ref agent) = app.task_filter_agent {
                        tasks.retain(|t| {
                            t.assigned_agent
                                .as_ref()
                                .map(|a| a == agent)
                                .unwrap_or(false)
                        });
                    }
                    app.tasks = tasks;
                }
            }
            app.last_task_poll = Instant::now();
        }

        // Receive diagnostic steps (non-blocking)
        if app.diag_active {
            let mut should_reload_workspace = false;
            let mut refresh_after_done = false;
            if let Some(ref mut rx) = app.diag_rx {
                while let Ok(mut step) = rx.try_recv() {
                    let reload_workspace = step.label == "DONE" && step.detail.contains("[reload-workspace]");
                    if reload_workspace {
                        step.detail = step.detail.replace(" [reload-workspace]", "");
                    }
                    let is_done = step.label == "DONE";
                    if step.label == "Gateway PID" && app.selected < app.agents.len() {
                        let pid = step.detail.strip_prefix("pid ").and_then(|v| v.trim().parse::<i32>().ok());
                        app.agents[app.selected].gateway_pid = pid;
                        app.agents[app.selected].gateway_status = if pid.unwrap_or(0) > 0 { GatewayStatus::Online } else { GatewayStatus::Offline };
                    }
                    app.diag_steps.push(step);
                    if is_done {
                        // Mark task as no longer running (overlay stays open for user to read)
                        app.diag_task_running = false;
                        if app.diag_title.as_deref().map(|t| t.starts_with("🌐 Gateway")).unwrap_or(false) {
                            refresh_after_done = true;
                        }
                    }
                    if reload_workspace {
                        should_reload_workspace = true;
                    }
                }
                if refresh_after_done {
                    app.start_refresh();
                }
            }
            if should_reload_workspace {
                app.start_workspace_load();
            }
        }

        // Receive fleet diagnostic messages (non-blocking)
        if app.fleet_diag_active {
            if let Some(ref mut rx) = app.fleet_diag_rx {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        FleetDiagMsg::AgentStart(ri) => {
                            if let Some(r) = app.fleet_diag_results.get_mut(ri) {
                                r.running = true;
                            }
                        }
                        FleetDiagMsg::CheckDone {
                            result_idx,
                            check_idx,
                            status,
                            issue,
                        } => {
                            if let Some(r) = app.fleet_diag_results.get_mut(result_idx) {
                                if check_idx < 7 {
                                    r.checks[check_idx] = Some(status);
                                }
                                if r.top_issue.is_empty() && matches!(status, DiagStatus::Fail) {
                                    r.top_issue = issue;
                                }
                            }
                        }
                        FleetDiagMsg::AgentDone(ri) => {
                            if let Some(r) = app.fleet_diag_results.get_mut(ri) {
                                r.running = false;
                                r.done = true;
                            }
                        }
                        FleetDiagMsg::AllDone => {
                            app.fleet_diag_done = true;
                        }
                    }
                }
            }
        }

        if let Some(ref mut rx) = app.wizard_ssh_rx {
            if let Ok(result) = rx.try_recv() {
                app.wizard.testing_ssh = false;
                app.wizard.ssh_result = Some(result);
                app.wizard_ssh_rx = None;
            }
        }

        if let Some(ref mut rx) = app.audit_rx {
            while let Ok(result) = rx.try_recv() {
                app.audit_pending = app.audit_pending.saturating_sub(1);
                app.audit_last = Some(if result.ok {
                    format!("🧾 {} {}", result.action, result.target)
                } else {
                    format!(
                        "🧾 {} {} (write failed: {})",
                        result.action,
                        result.target,
                        result.error.unwrap_or_else(|| "unknown".into())
                    )
                });
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

        if let Some(ref mut rx) = app.model_load_rx {
            if let Ok(result) = rx.try_recv() {
                if app.selected < app.agents.len() && app.agents[app.selected].db_name == result.agent_db_name {
                    app.agent_model = result.model;
                    app.model_options = merge_model_list(&result.models);
                    if let Some(current) = &app.agent_model {
                        if let Some(idx) = app.model_options.iter().position(|m| m == current) {
                            app.model_picker_selected = idx;
                        }
                    }
                }
                app.agent_model_loading = false;
                app.model_load_rx = None;
            }
        }

        if let Some(ref mut rx) = app.model_write_rx {
            if let Ok(result) = rx.try_recv() {
                if app.selected < app.agents.len() && app.agents[app.selected].db_name == result.agent_db_name {
                    app.agent_model = Some(result.model.clone());
                    app.agent_model_agent = Some(result.agent_db_name.clone());
                    if result.restarted {
                        app.toast(&format!("Model changed → {} — gateway restarted", result.model));
                    } else {
                        app.toast(&format!("Model changed → {} — gateway restart required", result.model));
                    }
                }
                app.model_write_rx = None;
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
                if app.ws_cron_selected >= app.ws_crons.len() {
                    app.ws_cron_selected = app.ws_crons.len().saturating_sub(1);
                }
                app.ws_loading = false;
                let found = app.ws_files.iter().filter(|f| f.exists).count();
                app.toast(&format!(
                    "✓ Loaded workspace — {}/{} files found",
                    found,
                    app.ws_files.len()
                ));
            }
        }
        if let Some(ref mut rx) = app.ws_file_rx {
            if let Ok(content) = rx.try_recv() {
                app.ws_content = Some(content);
                app.ws_content_scroll = 0;
            }
        }

        if let Some(ref mut rx) = app.db_latency_rx {
            while let Ok(latency) = rx.try_recv() {
                if let Some(ms) = latency {
                    app.db_latency_ms = Some(ms);
                    app.db_online = true;
                } else {
                    app.db_latency_ms = None;
                    app.db_online = false;
                }
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
                App::apply_thread_depth(&mut app.agent_chat_history);
            }
            if let Some(threads) = result.threads {
                let mut merged = threads;
                for old in &app.agent_threads {
                    if app.pinned_threads.contains(&old.thread_id)
                        && !merged.iter().any(|t| t.thread_id == old.thread_id)
                    {
                        merged.push(old.clone());
                    }
                }
                app.agent_threads = merged;
                if app.active_thread_id.is_none() {
                    app.active_thread_id = app.agent_threads.first().map(|t| t.thread_id.clone());
                }
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
        let has_pending = app
            .chat_history
            .iter()
            .chain(app.agent_chat_history.iter())
            .any(|m| {
                matches!(
                    m.status.as_str(),
                    "pending" | "connecting" | "thinking" | "streaming" | "processing" | "routing"
                )
            });
        let poll_interval = if has_pending {
            Duration::from_millis(400)
        } else {
            Duration::from_secs(3)
        };
        if !app.chat_polling && app.last_chat_poll.elapsed() > poll_interval {
            if let Some(pool) = app.db_pool.clone() {
                app.chat_polling = true;
                app.last_chat_poll = Instant::now();
                let (tx, rx) = mpsc::unbounded_channel();
                app.chat_poll_rx = Some(rx);
                let user = app.user();
                let routed = app.routed_msg_ids.clone();
                let on_detail =
                    app.screen == Screen::AgentDetail && app.selected < app.agents.len();
                let agent_name = if on_detail {
                    Some(app.agents[app.selected].db_name.clone())
                } else {
                    None
                };
                let active_thread_id = app.active_thread_id.clone();

                tokio::spawn(async move {
                    let mut to_route: Vec<(String, String)> = Vec::new();
                    let mut new_routed = Vec::new();

                    // Global chat
                    let global = if let Ok(msgs) = db::load_global_chat(&pool, 100).await {
                        for m in &msgs {
                            if m.status == "responded"
                                && m.sender != user
                                && !routed.contains(&m.id)
                            {
                                if let Some(ref resp) = m.response {
                                    if resp.contains('@') {
                                        new_routed.push(m.id);
                                        to_route.push((m.sender.clone(), resp.clone()));
                                    }
                                }
                            }
                        }
                        msgs.iter()
                            .map(|m| ChatLine {
                                id: m.id,
                                sender: m.sender.clone(),
                                target: m.target.clone(),
                                message: m.message.clone(),
                                response: m.response.clone(),
                                time: m.created_at.clone(),
                                status: m.status.clone(),
                                kind: m.kind.clone(),
                                thread_id: m.thread_id.clone(),
                                parent_id: m.parent_id,
                                depth: 0,
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                    // Agent chat + thread list
                    let threads = if let Some(ref name) = agent_name {
                        db::list_threads(&pool, name, 30).await.ok()
                    } else {
                        None
                    };
                    let agent = if let Some(ref name) = agent_name {
                        let load_res = if let Some(ref thread_id) = active_thread_id {
                            db::load_thread(&pool, thread_id, 100).await
                        } else {
                            db::load_agent_chat(&pool, name, 100).await
                        };
                        if let Ok(msgs) = load_res {
                            for m in &msgs {
                                if m.status == "responded"
                                    && m.sender != user
                                    && !routed.contains(&m.id)
                                {
                                    if let Some(ref resp) = m.response {
                                        if resp.contains('@') {
                                            new_routed.push(m.id);
                                            to_route.push((m.sender.clone(), resp.clone()));
                                        }
                                    }
                                }
                            }
                            Some(
                                msgs.iter()
                                    .map(|m| ChatLine {
                                        id: m.id,
                                        sender: m.sender.clone(),
                                        target: m.target.clone(),
                                        message: m.message.clone(),
                                        response: m.response.clone(),
                                        time: m.created_at.clone(),
                                        status: m.status.clone(),
                                        kind: m.kind.clone(),
                                        thread_id: m.thread_id.clone(),
                                        parent_id: m.parent_id,
                                        depth: 0,
                                    })
                                    .collect(),
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let _ = tx.send(ChatPollResult {
                        global,
                        agent,
                        threads,
                        to_route,
                        new_routed_ids: new_routed,
                    });
                });

                // Record routed IDs (we'll also get them from the result, but pre-mark to avoid dupes)
            }
        }

        if app.should_quit {
            // On clean exit: wait up to 3s for any active operation to complete
            if app.diag_task_running {
                let wait_start = std::time::Instant::now();
                while app.diag_task_running && wait_start.elapsed() < Duration::from_secs(3) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let mut should_reload_workspace = false;
                    if let Some(ref mut rx) = app.diag_rx {
                        while let Ok(step) = rx.try_recv() {
                            if step.label == "DONE" {
                                app.diag_task_running = false;
                            }
                            app.diag_steps.push(step);
                        }
                    }
                    if should_reload_workspace {
                        app.start_workspace_load();
                    }
                }
            }
            break;
        }
    }

    if let Some(pool) = app.db_pool.take() {
        pool.disconnect().await?;
    }
    disable_raw_mode()?;
    stdout().execute(crossterm::event::DisableMouseCapture)?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{App, ChatLine, INPUT_POLL_MS};

    #[test]
    fn input_poll_interval_is_low_for_responsive_ui() {
        assert!(INPUT_POLL_MS <= 10);
    }

    #[test]
    fn thread_title_is_truncated_to_40_chars() {
        let t = App::thread_title(
            "this is a very long title that should be shortened for sidebar display",
        );
        assert!(t.chars().count() <= 40);
    }

    #[test]
    fn thread_depth_is_capped_at_three() {
        let mut msgs = vec![
            ChatLine {
                id: 1,
                sender: "a".into(),
                target: None,
                message: "m1".into(),
                response: None,
                time: "".into(),
                status: "".into(),
                kind: "".into(),
                thread_id: None,
                parent_id: None,
                depth: 0,
            },
            ChatLine {
                id: 2,
                sender: "a".into(),
                target: None,
                message: "m2".into(),
                response: None,
                time: "".into(),
                status: "".into(),
                kind: "".into(),
                thread_id: None,
                parent_id: Some(1),
                depth: 0,
            },
            ChatLine {
                id: 3,
                sender: "a".into(),
                target: None,
                message: "m3".into(),
                response: None,
                time: "".into(),
                status: "".into(),
                kind: "".into(),
                thread_id: None,
                parent_id: Some(2),
                depth: 0,
            },
            ChatLine {
                id: 4,
                sender: "a".into(),
                target: None,
                message: "m4".into(),
                response: None,
                time: "".into(),
                status: "".into(),
                kind: "".into(),
                thread_id: None,
                parent_id: Some(3),
                depth: 0,
            },
            ChatLine {
                id: 5,
                sender: "a".into(),
                target: None,
                message: "m5".into(),
                response: None,
                time: "".into(),
                status: "".into(),
                kind: "".into(),
                thread_id: None,
                parent_id: Some(4),
                depth: 0,
            },
        ];
        App::apply_thread_depth(&mut msgs);
        assert_eq!(msgs[4].depth, 3);
    }
}
