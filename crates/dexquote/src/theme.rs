//! Presentation style resolution.
//!
//! A single source of truth for "should we emit ANSI colors and Unicode box
//! characters, or plain ASCII". The rules:
//!
//! 1. Explicit `--color always` / `--color never` or `display.color` in the
//!    config override everything.
//! 2. `NO_COLOR` environment variable forces color off (standard convention).
//! 3. If stdout is a real TTY: color + Unicode on.
//! 4. If stdout is piped: color + Unicode off.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "always" | "yes" | "on" => Self::Always,
            "never" | "no" | "off" => Self::Never,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub unicode: bool,
}

impl Theme {
    /// Resolve the theme from the given user preference plus environment.
    pub fn resolve(mode: ColorMode) -> Self {
        let color = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                if std::env::var_os("NO_COLOR").is_some() {
                    false
                } else {
                    std::io::stdout().is_terminal()
                }
            }
        };
        // Unicode tracks TTY-ness — pipes get ASCII for easy grep-ability.
        let unicode = std::io::stdout().is_terminal();
        Self { color, unicode }
    }

    /// Is stdout a live terminal we can draw spinners/streaming updates into?
    pub fn is_tty() -> bool {
        std::io::stdout().is_terminal()
    }

    pub fn sep_char(&self) -> char {
        if self.unicode {
            '─'
        } else {
            '-'
        }
    }

    pub fn arrow(&self) -> &'static str {
        if self.unicode {
            "→"
        } else {
            "->"
        }
    }

    pub fn star(&self) -> &'static str {
        if self.unicode {
            "★"
        } else {
            "*"
        }
    }

    pub fn dot(&self) -> &'static str {
        if self.unicode {
            "·"
        } else {
            "-"
        }
    }

    pub fn em_dash(&self) -> &'static str {
        if self.unicode {
            "—"
        } else {
            "--"
        }
    }
}
