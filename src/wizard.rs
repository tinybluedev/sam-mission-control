//! Interactive TUI wizard for adding new agents to the fleet.
//!
//! The wizard guides the operator through entering an agent name, display name,
//! emoji, host (Tailscale IP), SSH username, and location. On completion it calls
//! `sam onboard` to provision the agent over SSH.

use ratatui::prelude::*;
use ratatui::widgets::*;
use crate::theme::Theme;
use crate::validate;

#[derive(PartialEq, Clone)]
pub enum WizardStep {
    AgentName,
    DisplayName,
    Emoji,
    Host,
    SshUser,
    Location,
    Confirm,
}

impl WizardStep {
    pub fn next(&self) -> Option<Self> {
        match self {
            Self::AgentName => Some(Self::DisplayName),
            Self::DisplayName => Some(Self::Emoji),
            Self::Emoji => Some(Self::Host),
            Self::Host => Some(Self::SshUser),
            Self::SshUser => Some(Self::Location),
            Self::Location => Some(Self::Confirm),
            Self::Confirm => None,
        }
    }

    pub fn prev(&self) -> Option<Self> {
        match self {
            Self::AgentName => None,
            Self::DisplayName => Some(Self::AgentName),
            Self::Emoji => Some(Self::DisplayName),
            Self::Host => Some(Self::Emoji),
            Self::SshUser => Some(Self::Host),
            Self::Location => Some(Self::SshUser),
            Self::Confirm => Some(Self::Location),
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::AgentName => 0, Self::DisplayName => 1, Self::Emoji => 2,
            Self::Host => 3, Self::SshUser => 4, Self::Location => 5, Self::Confirm => 6,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::AgentName => "Agent Name",
            Self::DisplayName => "Display Name",
            Self::Emoji => "Emoji",
            Self::Host => "Host (IP/hostname)",
            Self::SshUser => "SSH User",
            Self::Location => "Location",
            Self::Confirm => "Confirm",
        }
    }
}

pub struct AgentWizard {
    pub active: bool,
    pub step: WizardStep,
    pub agent_name: String,
    pub display_name: String,
    pub emoji: String,
    pub host: String,
    pub ssh_user: String,
    pub location: usize, // index into LOCATIONS
    pub error: Option<String>,
    pub ssh_result: Option<String>,
    pub testing_ssh: bool,
}

pub const LOCATIONS: &[&str] = &["Home", "SM", "VPS", "Mobile"];

impl AgentWizard {
    pub fn new() -> Self {
        Self {
            active: false,
            step: WizardStep::AgentName,
            agent_name: String::new(),
            display_name: String::new(),
            emoji: "🤖".into(),
            host: String::new(),
            ssh_user: "admin".into(),
            location: 0,
            error: None,
            ssh_result: None,
            testing_ssh: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn open(&mut self) {
        self.reset();
        self.active = true;
    }

    pub fn current_input(&self) -> &str {
        match self.step {
            WizardStep::AgentName => &self.agent_name,
            WizardStep::DisplayName => &self.display_name,
            WizardStep::Emoji => &self.emoji,
            WizardStep::Host => &self.host,
            WizardStep::SshUser => &self.ssh_user,
            _ => "",
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.error = None;
        match self.step {
            WizardStep::AgentName => self.agent_name.push(c),
            WizardStep::DisplayName => self.display_name.push(c),
            WizardStep::Emoji => { self.emoji.clear(); self.emoji.push(c); }
            WizardStep::Host => self.host.push(c),
            WizardStep::SshUser => self.ssh_user.push(c),
            WizardStep::Location => {
                // Cycle location with any key
                self.location = (self.location + 1) % LOCATIONS.len();
            }
            _ => {}
        }
    }

    pub fn pop_char(&mut self) {
        match self.step {
            WizardStep::AgentName => { self.agent_name.pop(); }
            WizardStep::DisplayName => { self.display_name.pop(); }
            WizardStep::Emoji => {}
            WizardStep::Host => { self.host.pop(); }
            WizardStep::SshUser => { self.ssh_user.pop(); }
            _ => {}
        }
    }

    pub fn advance(&mut self) -> bool {
        // Validate current step
        match self.step {
            WizardStep::AgentName => {
                match validate::normalize_agent_name(&self.agent_name) {
                    Ok(name) => self.agent_name = name,
                    Err(e) => { self.error = Some(e); return false; }
                }
            }
            WizardStep::Host => {
                if let Err(e) = validate::validate_ip_address(self.host.trim()) {
                    self.error = Some(e);
                    return false;
                }
            }
            WizardStep::SshUser => {
                if let Err(e) = validate::validate_ssh_username(self.ssh_user.trim()) {
                    self.error = Some(e);
                    return false;
                }
            }
            WizardStep::Confirm => return true, // Signal: ready to create
            _ => {}
        }

        if let Some(next) = self.step.next() {
            // Auto-fill display name if empty
            if self.step == WizardStep::AgentName && self.display_name.is_empty() {
                self.display_name = self.agent_name.clone();
            }
            self.step = next;
        }
        false
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.step.prev() {
            self.step = prev;
            self.error = None;
            false
        } else {
            true // Signal: cancel wizard
        }
    }

    pub fn location_str(&self) -> &str {
        LOCATIONS[self.location]
    }
}

pub fn render_wizard(frame: &mut Frame, wizard: &AgentWizard, t: &Theme, bg: Color) {
    let area = frame.area();
    // Center modal: 60% width, 70% height
    let w = (area.width as f32 * 0.6) as u16;
    let h = (area.height as f32 * 0.7) as u16;
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    let modal = Rect::new(x, y, w, h);

    // Dim background
    let dim = Block::default().style(Style::default().bg(Color::Rgb(5, 5, 10)));
    frame.render_widget(dim, area);

    // Modal frame
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(8), Constraint::Length(3)])
        .split(modal);

    // Header
    let step_num = wizard.step.index() + 1;
    let header = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("🚀 New Agent", Style::default().fg(t.header_title).bold()),
        Span::raw("    "),
        Span::styled(format!("Step {}/7 — {}", step_num, wizard.step.label()), Style::default().fg(t.text_dim)),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_active)).style(Style::default().bg(bg)));
    frame.render_widget(header, inner[0]);

    // Body
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    // Progress bar
    let steps = ["Name", "Display", "Emoji", "Host", "SSH", "Location", "✓"];
    let progress: Vec<Span> = steps.iter().enumerate().map(|(i, s)| {
        let style = if i < wizard.step.index() {
            Style::default().fg(t.status_online)
        } else if i == wizard.step.index() {
            Style::default().fg(t.accent).bold()
        } else {
            Style::default().fg(t.text_dim)
        };
        let sep = if i < steps.len() - 1 { " → " } else { "" };
        Span::styled(format!("{}{}", s, sep), style)
    }).collect();
    lines.push(Line::from(vec![Span::raw("  ")]).into());
    lines.push(Line::from([vec![Span::raw("  ")], progress].concat()));
    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // Current field with all filled values
    let fields = vec![
        ("Agent Name", &wizard.agent_name, WizardStep::AgentName),
        ("Display Name", &wizard.display_name, WizardStep::DisplayName),
        ("Emoji", &wizard.emoji, WizardStep::Emoji),
        ("Host", &wizard.host, WizardStep::Host),
        ("SSH User", &wizard.ssh_user, WizardStep::SshUser),
    ];

    for (label, value, step) in &fields {
        let is_current = *step == wizard.step;
        let val_style = if is_current { Style::default().fg(t.accent).bold() } else { Style::default().fg(t.text) };
        let label_style = if is_current { Style::default().fg(t.accent).bold() } else { Style::default().fg(t.text_dim) };
        let cursor = if is_current { "▌" } else { "" };

        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(format!("{:<14}", label), label_style),
            Span::styled(if value.is_empty() { "(type here)" } else { value }, val_style),
            Span::styled(cursor, Style::default().fg(t.accent)),
        ]));
    }

    // Location (special — cycle with any key)
    let is_loc = wizard.step == WizardStep::Location;
    let loc_style = if is_loc { Style::default().fg(t.accent).bold() } else { Style::default().fg(t.text) };
    let loc_label = if is_loc { Style::default().fg(t.accent).bold() } else { Style::default().fg(t.text_dim) };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(format!("{:<14}", "Location"), loc_label),
        Span::styled(format!("{} (press any key to cycle)", wizard.location_str()), loc_style),
    ]));

    lines.push(Line::from(""));

    // Confirm step shows summary
    if wizard.step == WizardStep::Confirm {
        lines.push(Line::from(Span::styled("    ━━━ Ready to create ━━━", Style::default().fg(t.status_online).bold())));
        lines.push(Line::from(Span::styled("    Press Enter to confirm, Esc to go back", Style::default().fg(t.text_dim))));

        if let Some(result) = &wizard.ssh_result {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("SSH: ", Style::default().fg(t.text_bold).bold()),
                Span::styled(result.as_str(), Style::default().fg(t.response)),
            ]));
        }
    }

    // Error
    if let Some(err) = &wizard.error {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(format!("⚠️  {}", err), Style::default().fg(t.status_offline).bold()),
        ]));
    }

    let body = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.border_active)).style(Style::default().bg(bg))
            .padding(Padding::new(1, 1, 0, 0)));
    frame.render_widget(body, inner[1]);

    // Footer
    let footer_text = match wizard.step {
        WizardStep::Confirm => "Enter=create │ Esc=back │ Tab=test SSH",
        _ => "Enter=next │ Esc=back/cancel │ Tab=skip",
    };
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(footer_text, Style::default().fg(t.text_dim)),
    ])).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border)).style(Style::default().bg(bg)));
    frame.render_widget(footer, inner[2]);
}
