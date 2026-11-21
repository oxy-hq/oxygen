#![allow(dead_code)]

use colored::*;
use once_cell::sync::Lazy;
use std::sync::RwLock;

#[derive(Debug, Clone)]
struct Theme {
    primary: Color,
    secondary: Color,
    tertiary: Color,
    success: Color,
    warning: Color,
    error: Color,
    info: Color,
    text: Color,
}

#[derive(Debug, Clone)]
pub enum ThemeMode {
    Dark,
    Light,
}

struct ThemeManager {
    current_theme: Theme,
    mode: ThemeMode,
}

static THEME_MANAGER: Lazy<RwLock<ThemeManager>> = Lazy::new(|| RwLock::new(ThemeManager::new()));

impl Theme {
    fn dark() -> Self {
        Theme {
            primary: Color::TrueColor {
                r: 98,
                g: 186,
                b: 129,
            }, // muted green
            secondary: Color::TrueColor {
                r: 89,
                g: 134,
                b: 189,
            }, // muted blue
            tertiary: Color::TrueColor {
                r: 147,
                g: 108,
                b: 184,
            }, // muted purple
            success: Color::TrueColor {
                r: 98,
                g: 186,
                b: 129,
            }, // muted green
            warning: Color::TrueColor {
                r: 189,
                g: 179,
                b: 112,
            }, // muted yellow
            error: Color::TrueColor {
                r: 184,
                g: 108,
                b: 110,
            }, // muted red
            info: Color::TrueColor {
                r: 82,
                g: 172,
                b: 173,
            }, // muted emerald
            text: Color::TrueColor {
                r: 220,
                g: 220,
                b: 218,
            }, // slightly muted white
        }
    }

    fn light() -> Self {
        Theme {
            primary: Color::TrueColor {
                r: 64,
                g: 202,
                b: 82,
            }, // green
            secondary: Color::TrueColor {
                r: 27,
                g: 143,
                b: 239,
            }, // blue
            tertiary: Color::TrueColor {
                r: 214,
                g: 105,
                b: 245,
            }, // purple
            success: Color::TrueColor {
                r: 64,
                g: 202,
                b: 82,
            },
            warning: Color::TrueColor {
                r: 221,
                g: 185,
                b: 87,
            }, // yellow
            error: Color::TrueColor {
                r: 221,
                g: 96,
                b: 94,
            }, // red
            info: Color::TrueColor {
                r: 16,
                g: 193,
                b: 167,
            }, // emerald
            text: Color::TrueColor {
                r: 53,
                g: 55,
                b: 58,
            }, // gray
        }
    }

    fn legacy() -> Self {
        Theme {
            primary: Color::Green,
            secondary: Color::Blue,
            tertiary: Color::Magenta,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            info: Color::Cyan,
            text: Color::White,
        }
    }
}

impl ThemeManager {
    fn new() -> Self {
        let true_color = Self::detect_true_color_support();
        let mode = Self::detect_terminal_theme();
        let mut current_theme = match mode {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
        };
        if !true_color {
            current_theme = Theme::legacy()
        }
        ThemeManager {
            current_theme,
            mode,
        }
    }

    fn detect_terminal_theme() -> ThemeMode {
        match terminal_light::luma() {
            Ok(luma) if luma > 0.85 => ThemeMode::Light,
            Ok(luma) if luma < 0.2 => ThemeMode::Dark,
            _ => ThemeMode::Dark,
        }
    }

    fn detect_true_color_support() -> bool {
        std::env::var("COLORTERM")
            .map(|v| v.to_lowercase() == "truecolor")
            .unwrap_or(false)
    }

    fn get_instance() -> &'static RwLock<ThemeManager> {
        &THEME_MANAGER
    }

    fn switch_theme(&mut self, mode: ThemeMode) {
        self.mode = mode.clone();
        self.current_theme = match mode {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
        };
    }
}

pub fn get_current_theme_mode() -> ThemeMode {
    ThemeManager::get_instance().read().unwrap().mode.clone()
}

pub fn detect_true_color_support() -> bool {
    ThemeManager::detect_true_color_support()
}

pub fn switch_theme(mode: ThemeMode) {
    ThemeManager::get_instance()
        .write()
        .unwrap()
        .switch_theme(mode);
}

pub trait StyledText {
    fn primary(self) -> ColoredString;
    fn secondary(self) -> ColoredString;
    fn tertiary(self) -> ColoredString;
    fn success(self) -> ColoredString;
    fn warning(self) -> ColoredString;
    fn error(self) -> ColoredString;
    fn info(self) -> ColoredString;
    fn text(self) -> ColoredString;
}

impl StyledText for &str {
    fn primary(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.primary)
    }

    fn secondary(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.secondary)
    }

    fn tertiary(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.tertiary)
    }

    fn success(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.success)
    }

    fn warning(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.warning)
    }

    fn error(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.error)
    }

    fn info(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.info)
    }

    fn text(self) -> ColoredString {
        let theme_manager = ThemeManager::get_instance().read().unwrap();
        self.color(theme_manager.current_theme.text)
    }
}
