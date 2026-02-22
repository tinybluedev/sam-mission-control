//! Color theme definitions for the S.A.M Mission Control TUI.
//!
//! Each theme is a [`Theme`] struct containing named color fields used throughout
//! the UI. Available themes: `standard`, `noir`, `paper`, `1977`, `2077`,
//! `matrix`, `sunset`, `arctic`.

use ratatui::prelude::*;
use ratatui::widgets::BorderType;

// ---- Background Density ----

#[derive(Clone, Copy, PartialEq)]
pub enum BgDensity {
    Dark,       // Default — near black
    Medium,     // Slightly lighter
    Light,      // Visible grey
    White,      // Light mode
    Transparent, // Pure terminal default
}

impl BgDensity {
    pub fn next(self) -> Self {
        match self {
            Self::Dark => Self::Medium,
            Self::Medium => Self::Light,
            Self::Light => Self::White,
            Self::White => Self::Transparent,
            Self::Transparent => Self::Dark,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Dark => "dark",
            Self::Medium => "medium",
            Self::Light => "light",
            Self::White => "white",
            Self::Transparent => "terminal",
        }
    }

    pub fn bg(&self) -> Color {
        match self {
            Self::Dark => Color::Rgb(10, 10, 18),
            Self::Medium => Color::Rgb(30, 30, 40),
            Self::Light => Color::Rgb(60, 60, 70),
            Self::White => Color::Rgb(230, 230, 235),
            Self::Transparent => Color::Reset,
        }
    }

    pub fn is_light(&self) -> bool {
        matches!(self, Self::White)
    }
}

// ---- Color Themes ----

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeName {
    Standard,   // Original cyan/blue
    Noir,       // All white/grey on black
    Paper,      // All black on white
    Retro1977,  // Warm amber/orange/brown
    Cyber2077,  // Neon pink/cyan/yellow
    Matrix,     // Green on black
    Sunset,     // Warm orange/red/purple
    Arctic,     // Cool blue/white/silver
    Ocean,      // Blue/teal/white
    Ember,      // Orange/red/dark
}

impl ThemeName {
    pub fn next(self) -> Self {
        match self {
            Self::Standard => Self::Noir,
            Self::Noir => Self::Paper,
            Self::Paper => Self::Retro1977,
            Self::Retro1977 => Self::Cyber2077,
            Self::Cyber2077 => Self::Matrix,
            Self::Matrix => Self::Sunset,
            Self::Sunset => Self::Arctic,
            Self::Arctic => Self::Ocean,
            Self::Ocean => Self::Ember,
            Self::Ember => Self::Standard,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Standard => "standard",
            Self::Noir => "noir",
            Self::Paper => "paper",
            Self::Retro1977 => "1977",
            Self::Cyber2077 => "2077",
            Self::Matrix => "matrix",
            Self::Sunset => "sunset",
            Self::Arctic => "arctic",
            Self::Ocean => "ocean",
            Self::Ember => "ember",
        }
    }
}

// The actual resolved colors for rendering
#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,         // Primary accent (borders, highlights)
    pub accent2: Color,        // Secondary accent
    pub text: Color,           // Main text
    pub text_dim: Color,       // Dimmed text (timestamps, labels)
    pub text_bold: Color,      // Bold/emphasized text
    pub border: Color,         // Inactive borders
    pub border_active: Color,  // Active borders
    pub selected_bg: Color,    // Selected row background
    pub sender_self: Color,    // Chat: own messages
    pub sender_other: Color,   // Chat: other messages
    pub response: Color,       // Chat: response text
    pub status_online: Color,
    pub status_busy: Color,
    pub status_offline: Color,
    pub loc_home: Color,
    pub loc_sm: Color,
    pub loc_vps: Color,
    pub loc_mobile: Color,
    pub version: Color,
    pub pending: Color,        // Awaiting response
    pub header_title: Color,
    pub border_type: BorderType, // Border style for this theme
}

impl Theme {
    pub fn resolve(name: ThemeName, bg: BgDensity) -> Self {
        let light = bg.is_light();
        match name {
            ThemeName::Standard => if light { Self::standard_light() } else { Self::standard() },
            ThemeName::Noir => if light { Self::paper() } else { Self::noir() },
            ThemeName::Paper => Self::paper(),
            ThemeName::Retro1977 => Self::retro1977(light),
            ThemeName::Cyber2077 => Self::cyber2077(light),
            ThemeName::Matrix => Self::matrix(light),
            ThemeName::Sunset => Self::sunset(light),
            ThemeName::Arctic => Self::arctic(light),
            ThemeName::Ocean => Self::ocean(light),
            ThemeName::Ember => Self::ember(light),
        }
    }

    fn standard() -> Self {
        Self {
            accent: Color::Rgb(80, 200, 255),
            accent2: Color::Rgb(120, 200, 220),
            text: Color::Rgb(220, 220, 230),
            text_dim: Color::Rgb(80, 80, 100),
            text_bold: Color::White,
            border: Color::Rgb(50, 50, 70),
            border_active: Color::Rgb(80, 200, 255),
            selected_bg: Color::Rgb(35, 40, 60),
            sender_self: Color::Rgb(230, 180, 60),
            sender_other: Color::Rgb(100, 220, 100),
            response: Color::Rgb(160, 210, 170),
            status_online: Color::Green,
            status_busy: Color::Yellow,
            status_offline: Color::Red,
            loc_home: Color::Rgb(100, 200, 100),
            loc_sm: Color::Rgb(230, 180, 60),
            loc_vps: Color::Rgb(180, 120, 220),
            loc_mobile: Color::Rgb(80, 180, 230),
            version: Color::Rgb(120, 200, 220),
            pending: Color::Rgb(100, 100, 120),
            header_title: Color::Rgb(80, 200, 255),
            border_type: BorderType::Rounded,
        }
    }

    fn standard_light() -> Self {
        Self {
            accent: Color::Rgb(0, 120, 180),
            accent2: Color::Rgb(0, 100, 160),
            text: Color::Rgb(30, 30, 40),
            text_dim: Color::Rgb(100, 100, 120),
            text_bold: Color::Black,
            border: Color::Rgb(180, 180, 190),
            border_active: Color::Rgb(0, 120, 180),
            selected_bg: Color::Rgb(210, 225, 240),
            sender_self: Color::Rgb(180, 120, 0),
            sender_other: Color::Rgb(0, 140, 60),
            response: Color::Rgb(0, 120, 80),
            status_online: Color::Rgb(0, 150, 0),
            status_busy: Color::Rgb(200, 150, 0),
            status_offline: Color::Rgb(200, 0, 0),
            loc_home: Color::Rgb(0, 140, 60),
            loc_sm: Color::Rgb(180, 120, 0),
            loc_vps: Color::Rgb(120, 60, 180),
            loc_mobile: Color::Rgb(0, 120, 180),
            version: Color::Rgb(0, 100, 160),
            pending: Color::Rgb(140, 140, 160),
            header_title: Color::Rgb(0, 120, 180),
            border_type: BorderType::Rounded,
        }
    }

    fn noir() -> Self {
        Self {
            accent: Color::White,
            accent2: Color::Rgb(180, 180, 180),
            text: Color::Rgb(200, 200, 200),
            text_dim: Color::Rgb(80, 80, 80),
            text_bold: Color::White,
            border: Color::Rgb(40, 40, 40),
            border_active: Color::White,
            selected_bg: Color::Rgb(30, 30, 30),
            sender_self: Color::White,
            sender_other: Color::Rgb(160, 160, 160),
            response: Color::Rgb(140, 140, 140),
            status_online: Color::White,
            status_busy: Color::Rgb(160, 160, 160),
            status_offline: Color::Rgb(60, 60, 60),
            loc_home: Color::Rgb(180, 180, 180),
            loc_sm: Color::Rgb(160, 160, 160),
            loc_vps: Color::Rgb(140, 140, 140),
            loc_mobile: Color::Rgb(120, 120, 120),
            version: Color::Rgb(180, 180, 180),
            pending: Color::Rgb(80, 80, 80),
            header_title: Color::White,
            border_type: BorderType::Plain,
        }
    }

    fn paper() -> Self {
        Self {
            accent: Color::Black,
            accent2: Color::Rgb(60, 60, 60),
            text: Color::Rgb(30, 30, 30),
            text_dim: Color::Rgb(120, 120, 120),
            text_bold: Color::Black,
            border: Color::Rgb(180, 180, 180),
            border_active: Color::Black,
            selected_bg: Color::Rgb(220, 220, 225),
            sender_self: Color::Black,
            sender_other: Color::Rgb(60, 60, 60),
            response: Color::Rgb(80, 80, 80),
            status_online: Color::Rgb(0, 100, 0),
            status_busy: Color::Rgb(150, 100, 0),
            status_offline: Color::Rgb(160, 0, 0),
            loc_home: Color::Rgb(0, 100, 0),
            loc_sm: Color::Rgb(150, 100, 0),
            loc_vps: Color::Rgb(100, 50, 150),
            loc_mobile: Color::Rgb(0, 80, 160),
            version: Color::Rgb(60, 60, 60),
            pending: Color::Rgb(140, 140, 140),
            header_title: Color::Black,
            border_type: BorderType::Plain,
        }
    }

    fn retro1977(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(160, 80, 0),
                accent2: Color::Rgb(140, 70, 0),
                text: Color::Rgb(60, 40, 20),
                text_dim: Color::Rgb(140, 120, 90),
                text_bold: Color::Rgb(100, 50, 0),
                border: Color::Rgb(190, 170, 140),
                border_active: Color::Rgb(160, 80, 0),
                selected_bg: Color::Rgb(230, 210, 180),
                sender_self: Color::Rgb(160, 80, 0),
                sender_other: Color::Rgb(120, 80, 20),
                response: Color::Rgb(100, 70, 30),
                status_online: Color::Rgb(80, 120, 0),
                status_busy: Color::Rgb(180, 120, 0),
                status_offline: Color::Rgb(160, 40, 0),
                loc_home: Color::Rgb(80, 120, 0),
                loc_sm: Color::Rgb(180, 120, 0),
                loc_vps: Color::Rgb(140, 60, 100),
                loc_mobile: Color::Rgb(0, 100, 140),
                version: Color::Rgb(140, 70, 0),
                pending: Color::Rgb(160, 140, 110),
                header_title: Color::Rgb(160, 80, 0),
                border_type: BorderType::Plain,
            }
        } else {
            Self {
                accent: Color::Rgb(255, 170, 50),
                accent2: Color::Rgb(220, 140, 40),
                text: Color::Rgb(230, 200, 160),
                text_dim: Color::Rgb(120, 90, 50),
                text_bold: Color::Rgb(255, 200, 100),
                border: Color::Rgb(80, 60, 30),
                border_active: Color::Rgb(255, 170, 50),
                selected_bg: Color::Rgb(50, 35, 15),
                sender_self: Color::Rgb(255, 170, 50),
                sender_other: Color::Rgb(200, 160, 80),
                response: Color::Rgb(180, 150, 100),
                status_online: Color::Rgb(180, 220, 80),
                status_busy: Color::Rgb(255, 180, 40),
                status_offline: Color::Rgb(200, 60, 30),
                loc_home: Color::Rgb(180, 220, 80),
                loc_sm: Color::Rgb(255, 180, 40),
                loc_vps: Color::Rgb(200, 120, 160),
                loc_mobile: Color::Rgb(100, 180, 220),
                version: Color::Rgb(220, 140, 40),
                pending: Color::Rgb(120, 90, 50),
                header_title: Color::Rgb(255, 170, 50),
                border_type: BorderType::Plain,
            }
        }
    }

    fn cyber2077(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(200, 0, 120),
                accent2: Color::Rgb(0, 160, 200),
                text: Color::Rgb(30, 20, 40),
                text_dim: Color::Rgb(120, 100, 140),
                text_bold: Color::Rgb(180, 0, 100),
                border: Color::Rgb(180, 170, 190),
                border_active: Color::Rgb(200, 0, 120),
                selected_bg: Color::Rgb(240, 220, 240),
                sender_self: Color::Rgb(200, 0, 120),
                sender_other: Color::Rgb(0, 140, 180),
                response: Color::Rgb(0, 120, 160),
                status_online: Color::Rgb(0, 200, 100),
                status_busy: Color::Rgb(220, 180, 0),
                status_offline: Color::Rgb(200, 0, 60),
                loc_home: Color::Rgb(0, 160, 80),
                loc_sm: Color::Rgb(200, 150, 0),
                loc_vps: Color::Rgb(160, 0, 200),
                loc_mobile: Color::Rgb(0, 140, 200),
                version: Color::Rgb(0, 160, 200),
                pending: Color::Rgb(140, 120, 160),
                header_title: Color::Rgb(200, 0, 120),
                border_type: BorderType::Thick,
            }
        } else {
            Self {
                accent: Color::Rgb(255, 0, 150),
                accent2: Color::Rgb(0, 255, 255),
                text: Color::Rgb(230, 220, 240),
                text_dim: Color::Rgb(100, 80, 120),
                text_bold: Color::Rgb(255, 255, 100),
                border: Color::Rgb(60, 30, 80),
                border_active: Color::Rgb(255, 0, 150),
                selected_bg: Color::Rgb(40, 15, 50),
                sender_self: Color::Rgb(255, 255, 100),
                sender_other: Color::Rgb(0, 255, 255),
                response: Color::Rgb(0, 200, 200),
                status_online: Color::Rgb(0, 255, 100),
                status_busy: Color::Rgb(255, 255, 0),
                status_offline: Color::Rgb(255, 0, 60),
                loc_home: Color::Rgb(0, 255, 100),
                loc_sm: Color::Rgb(255, 200, 0),
                loc_vps: Color::Rgb(200, 0, 255),
                loc_mobile: Color::Rgb(0, 200, 255),
                version: Color::Rgb(0, 255, 255),
                pending: Color::Rgb(100, 80, 120),
                header_title: Color::Rgb(255, 0, 150),
                border_type: BorderType::Thick,
            }
        }
    }

    fn matrix(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(0, 120, 0),
                accent2: Color::Rgb(0, 100, 0),
                text: Color::Rgb(0, 60, 0),
                text_dim: Color::Rgb(80, 140, 80),
                text_bold: Color::Rgb(0, 100, 0),
                border: Color::Rgb(140, 180, 140),
                border_active: Color::Rgb(0, 120, 0),
                selected_bg: Color::Rgb(210, 235, 210),
                sender_self: Color::Rgb(0, 120, 0),
                sender_other: Color::Rgb(0, 80, 0),
                response: Color::Rgb(0, 100, 0),
                status_online: Color::Rgb(0, 140, 0),
                status_busy: Color::Rgb(100, 140, 0),
                status_offline: Color::Rgb(140, 0, 0),
                loc_home: Color::Rgb(0, 120, 0),
                loc_sm: Color::Rgb(0, 100, 0),
                loc_vps: Color::Rgb(0, 80, 0),
                loc_mobile: Color::Rgb(0, 60, 0),
                version: Color::Rgb(0, 100, 0),
                pending: Color::Rgb(80, 140, 80),
                header_title: Color::Rgb(0, 120, 0),
                border_type: BorderType::Plain,
            }
        } else {
            Self {
                accent: Color::Rgb(0, 255, 0),
                accent2: Color::Rgb(0, 200, 0),
                text: Color::Rgb(0, 220, 0),
                text_dim: Color::Rgb(0, 80, 0),
                text_bold: Color::Rgb(0, 255, 0),
                border: Color::Rgb(0, 50, 0),
                border_active: Color::Rgb(0, 255, 0),
                selected_bg: Color::Rgb(0, 25, 0),
                sender_self: Color::Rgb(0, 255, 0),
                sender_other: Color::Rgb(0, 180, 0),
                response: Color::Rgb(0, 160, 0),
                status_online: Color::Rgb(0, 255, 0),
                status_busy: Color::Rgb(0, 200, 0),
                status_offline: Color::Rgb(0, 60, 0),
                loc_home: Color::Rgb(0, 220, 0),
                loc_sm: Color::Rgb(0, 200, 0),
                loc_vps: Color::Rgb(0, 180, 0),
                loc_mobile: Color::Rgb(0, 160, 0),
                version: Color::Rgb(0, 200, 0),
                pending: Color::Rgb(0, 80, 0),
                header_title: Color::Rgb(0, 255, 0),
                border_type: BorderType::Plain,
            }
        }
    }

    fn sunset(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(200, 60, 40),
                accent2: Color::Rgb(180, 80, 120),
                text: Color::Rgb(60, 30, 30),
                text_dim: Color::Rgb(160, 120, 110),
                text_bold: Color::Rgb(180, 40, 20),
                border: Color::Rgb(200, 180, 170),
                border_active: Color::Rgb(200, 60, 40),
                selected_bg: Color::Rgb(240, 220, 210),
                sender_self: Color::Rgb(200, 60, 40),
                sender_other: Color::Rgb(160, 80, 120),
                response: Color::Rgb(140, 70, 100),
                status_online: Color::Rgb(80, 160, 0),
                status_busy: Color::Rgb(220, 140, 0),
                status_offline: Color::Rgb(180, 0, 0),
                loc_home: Color::Rgb(80, 140, 0),
                loc_sm: Color::Rgb(200, 120, 0),
                loc_vps: Color::Rgb(160, 40, 120),
                loc_mobile: Color::Rgb(0, 100, 180),
                version: Color::Rgb(180, 80, 120),
                pending: Color::Rgb(160, 130, 120),
                header_title: Color::Rgb(200, 60, 40),
                border_type: BorderType::Rounded,
            }
        } else {
            Self {
                accent: Color::Rgb(255, 100, 50),
                accent2: Color::Rgb(255, 80, 160),
                text: Color::Rgb(240, 210, 200),
                text_dim: Color::Rgb(120, 80, 60),
                text_bold: Color::Rgb(255, 200, 100),
                border: Color::Rgb(80, 40, 30),
                border_active: Color::Rgb(255, 100, 50),
                selected_bg: Color::Rgb(50, 25, 15),
                sender_self: Color::Rgb(255, 200, 100),
                sender_other: Color::Rgb(255, 120, 180),
                response: Color::Rgb(220, 100, 150),
                status_online: Color::Rgb(255, 200, 80),
                status_busy: Color::Rgb(255, 140, 40),
                status_offline: Color::Rgb(140, 40, 30),
                loc_home: Color::Rgb(255, 180, 80),
                loc_sm: Color::Rgb(255, 140, 40),
                loc_vps: Color::Rgb(200, 80, 180),
                loc_mobile: Color::Rgb(100, 160, 240),
                version: Color::Rgb(255, 80, 160),
                pending: Color::Rgb(120, 80, 60),
                header_title: Color::Rgb(255, 100, 50),
                border_type: BorderType::Rounded,
            }
        }
    }

    fn arctic(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(0, 80, 160),
                accent2: Color::Rgb(60, 120, 180),
                text: Color::Rgb(20, 30, 50),
                text_dim: Color::Rgb(100, 120, 150),
                text_bold: Color::Rgb(0, 60, 140),
                border: Color::Rgb(170, 185, 200),
                border_active: Color::Rgb(0, 80, 160),
                selected_bg: Color::Rgb(210, 225, 245),
                sender_self: Color::Rgb(0, 80, 160),
                sender_other: Color::Rgb(60, 120, 180),
                response: Color::Rgb(40, 100, 160),
                status_online: Color::Rgb(0, 140, 100),
                status_busy: Color::Rgb(160, 140, 0),
                status_offline: Color::Rgb(160, 40, 40),
                loc_home: Color::Rgb(0, 120, 80),
                loc_sm: Color::Rgb(140, 120, 0),
                loc_vps: Color::Rgb(100, 60, 160),
                loc_mobile: Color::Rgb(0, 100, 180),
                version: Color::Rgb(60, 120, 180),
                pending: Color::Rgb(120, 140, 160),
                header_title: Color::Rgb(0, 80, 160),
                border_type: BorderType::Rounded,
            }
        } else {
            Self {
                accent: Color::Rgb(120, 180, 255),
                accent2: Color::Rgb(180, 210, 255),
                text: Color::Rgb(210, 225, 245),
                text_dim: Color::Rgb(80, 100, 130),
                text_bold: Color::White,
                border: Color::Rgb(40, 50, 70),
                border_active: Color::Rgb(120, 180, 255),
                selected_bg: Color::Rgb(25, 35, 55),
                sender_self: Color::Rgb(180, 210, 255),
                sender_other: Color::Rgb(120, 180, 255),
                response: Color::Rgb(140, 190, 240),
                status_online: Color::Rgb(100, 220, 180),
                status_busy: Color::Rgb(220, 200, 100),
                status_offline: Color::Rgb(180, 80, 80),
                loc_home: Color::Rgb(100, 200, 160),
                loc_sm: Color::Rgb(200, 180, 100),
                loc_vps: Color::Rgb(160, 120, 220),
                loc_mobile: Color::Rgb(100, 180, 240),
                version: Color::Rgb(180, 210, 255),
                pending: Color::Rgb(80, 100, 130),
                header_title: Color::Rgb(120, 180, 255),
                border_type: BorderType::Rounded,
            }
        }
    }

    fn ocean(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(0, 130, 160),
                accent2: Color::Rgb(0, 170, 190),
                text: Color::Rgb(10, 40, 60),
                text_dim: Color::Rgb(80, 130, 150),
                text_bold: Color::Rgb(0, 100, 130),
                border: Color::Rgb(160, 210, 220),
                border_active: Color::Rgb(0, 130, 160),
                selected_bg: Color::Rgb(200, 235, 240),
                sender_self: Color::Rgb(0, 130, 160),
                sender_other: Color::Rgb(0, 160, 140),
                response: Color::Rgb(0, 140, 120),
                status_online: Color::Rgb(0, 160, 100),
                status_busy: Color::Rgb(180, 150, 0),
                status_offline: Color::Rgb(160, 40, 40),
                loc_home: Color::Rgb(0, 140, 100),
                loc_sm: Color::Rgb(160, 130, 0),
                loc_vps: Color::Rgb(80, 60, 160),
                loc_mobile: Color::Rgb(0, 120, 180),
                version: Color::Rgb(0, 160, 180),
                pending: Color::Rgb(100, 150, 160),
                header_title: Color::Rgb(0, 130, 160),
                border_type: BorderType::Rounded,
            }
        } else {
            Self {
                accent: Color::Rgb(0, 200, 230),
                accent2: Color::Rgb(0, 230, 200),
                text: Color::Rgb(200, 235, 240),
                text_dim: Color::Rgb(60, 110, 130),
                text_bold: Color::White,
                border: Color::Rgb(20, 60, 80),
                border_active: Color::Rgb(0, 200, 230),
                selected_bg: Color::Rgb(10, 40, 60),
                sender_self: Color::Rgb(0, 230, 200),
                sender_other: Color::Rgb(0, 200, 230),
                response: Color::Rgb(0, 180, 210),
                status_online: Color::Rgb(0, 230, 160),
                status_busy: Color::Rgb(220, 200, 60),
                status_offline: Color::Rgb(200, 70, 70),
                loc_home: Color::Rgb(0, 220, 160),
                loc_sm: Color::Rgb(200, 190, 60),
                loc_vps: Color::Rgb(140, 100, 230),
                loc_mobile: Color::Rgb(60, 180, 240),
                version: Color::Rgb(0, 210, 230),
                pending: Color::Rgb(60, 110, 130),
                header_title: Color::Rgb(0, 200, 230),
                border_type: BorderType::Rounded,
            }
        }
    }

    fn ember(light: bool) -> Self {
        if light {
            Self {
                accent: Color::Rgb(200, 80, 20),
                accent2: Color::Rgb(220, 120, 0),
                text: Color::Rgb(60, 25, 10),
                text_dim: Color::Rgb(160, 110, 80),
                text_bold: Color::Rgb(180, 60, 0),
                border: Color::Rgb(210, 175, 155),
                border_active: Color::Rgb(200, 80, 20),
                selected_bg: Color::Rgb(245, 220, 200),
                sender_self: Color::Rgb(200, 80, 20),
                sender_other: Color::Rgb(180, 100, 0),
                response: Color::Rgb(160, 80, 0),
                status_online: Color::Rgb(100, 150, 0),
                status_busy: Color::Rgb(220, 140, 0),
                status_offline: Color::Rgb(180, 20, 0),
                loc_home: Color::Rgb(100, 140, 0),
                loc_sm: Color::Rgb(200, 130, 0),
                loc_vps: Color::Rgb(160, 40, 100),
                loc_mobile: Color::Rgb(0, 110, 170),
                version: Color::Rgb(200, 100, 0),
                pending: Color::Rgb(170, 130, 100),
                header_title: Color::Rgb(200, 80, 20),
                border_type: BorderType::Double,
            }
        } else {
            Self {
                accent: Color::Rgb(255, 120, 40),
                accent2: Color::Rgb(255, 80, 30),
                text: Color::Rgb(240, 200, 170),
                text_dim: Color::Rgb(110, 65, 35),
                text_bold: Color::Rgb(255, 180, 80),
                border: Color::Rgb(80, 35, 15),
                border_active: Color::Rgb(255, 120, 40),
                selected_bg: Color::Rgb(55, 25, 10),
                sender_self: Color::Rgb(255, 200, 80),
                sender_other: Color::Rgb(255, 100, 50),
                response: Color::Rgb(220, 80, 30),
                status_online: Color::Rgb(200, 220, 60),
                status_busy: Color::Rgb(255, 160, 20),
                status_offline: Color::Rgb(160, 30, 10),
                loc_home: Color::Rgb(200, 210, 60),
                loc_sm: Color::Rgb(255, 150, 20),
                loc_vps: Color::Rgb(210, 70, 160),
                loc_mobile: Color::Rgb(80, 160, 240),
                version: Color::Rgb(255, 140, 40),
                pending: Color::Rgb(110, 65, 35),
                header_title: Color::Rgb(255, 120, 40),
                border_type: BorderType::Double,
            }
        }
    }
}
