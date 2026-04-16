//! Keyboard event → high-level `Intent` mapping.
//!
//! The rest of the TUI reasons in semantic intents (`Next`, `Activate`,
//! `Character`) instead of raw key codes, so rebinding or adding a second
//! input mode later is a change in this file only.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Field, Phase};

#[derive(Debug, Clone)]
pub enum Intent {
    Quit,
    Next,
    Prev,
    Up,
    Down,
    Activate,
    /// v1.2 list-view navigation: jump by a page at a time.
    PageUp,
    PageDown,
    /// v1.2 list-view navigation: jump to the first/last row.
    Home,
    End,
    /// Ctrl+Enter — fire the quote regardless of which field is focused.
    QuoteNow,
    /// `s` — swap sell ↔ buy tokens without changing amount or fields.
    SwapTokens,
    /// `y` — copy the best-quote line to the system clipboard.
    YankBest,
    /// `?` — toggle the keybindings help overlay.
    ToggleHelp,
    Character(char),
    Backspace,
    Ignore,
}

pub fn map_key(key: KeyEvent, app: &App) -> Intent {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        return Intent::Quit;
    }

    // Ctrl+Enter: global "fire quote from anywhere" shortcut for power users.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Enter) {
        return Intent::QuoteNow;
    }

    match key.code {
        KeyCode::Esc => Intent::Quit,
        KeyCode::Tab => Intent::Next,
        KeyCode::BackTab => Intent::Prev,
        KeyCode::Enter => Intent::Activate,
        KeyCode::Up => {
            // TokenPicker, MainMenu, ChainPicker, and the v1.2 list
            // views all treat Up/Down as list navigation rather than
            // field-focus cycling.
            if matches!(
                app.phase,
                Phase::TokenPicker
                    | Phase::MainMenu
                    | Phase::ChainPicker
                    | Phase::ShowingTokens
                    | Phase::ShowingHistory
            ) {
                Intent::Up
            } else {
                Intent::Prev
            }
        }
        KeyCode::Down => {
            if matches!(
                app.phase,
                Phase::TokenPicker
                    | Phase::MainMenu
                    | Phase::ChainPicker
                    | Phase::ShowingTokens
                    | Phase::ShowingHistory
            ) {
                Intent::Down
            } else {
                Intent::Next
            }
        }
        KeyCode::Left | KeyCode::Right => Intent::Ignore,
        KeyCode::PageUp => Intent::PageUp,
        KeyCode::PageDown => Intent::PageDown,
        KeyCode::Home => Intent::Home,
        KeyCode::End => Intent::End,
        KeyCode::Backspace => Intent::Backspace,
        KeyCode::Char(c) => {
            // In field-editing phase, only forward digits/dot to the amount
            // field; a-z navigates nothing meaningful. In picker/custom
            // phases, forward every printable character to the filter input.
            // In MainMenu / ChainPicker, forward digits (jump shortcuts) and
            // j/k (vim navigation).
            match app.phase {
                Phase::MainMenu | Phase::ChainPicker => {
                    if c.is_ascii_digit() {
                        Intent::Character(c)
                    } else if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == 'j' {
                        Intent::Down
                    } else if c == 'k' {
                        Intent::Up
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::ShowingTokens | Phase::ShowingHistory => {
                    if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == 'j' {
                        Intent::Down
                    } else if c == 'k' {
                        Intent::Up
                    } else if c == 'g' {
                        Intent::Home
                    } else if c == 'G' {
                        Intent::End
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::TokenPicker => Intent::Character(c),
                Phase::CustomAddressEntry => Intent::Character(c),
                Phase::EditingFields | Phase::ShowingResults | Phase::ShowingRoute => {
                    if app.focus == Field::Amount && (c.is_ascii_digit() || c == '.') {
                        Intent::Character(c)
                    } else if c == 'q' {
                        Intent::Quit
                    } else if c == 'r' || c == 'R' {
                        // `R` = re-run with the current inputs. Works in any
                        // focus when fields are populated, giving power users
                        // a one-key refresh while looking at results.
                        Intent::QuoteNow
                    } else if c == 's' || c == 'S' {
                        Intent::SwapTokens
                    } else if c == 'y' || c == 'Y' {
                        Intent::YankBest
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::Quoting => Intent::Ignore,
                Phase::Depthing => {
                    if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::ShowingDepth => {
                    if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::Benchmarking | Phase::ShowingBenchmark => {
                    if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
                Phase::Doctoring | Phase::ShowingDoctor => {
                    if c == 'q' || c == 'Q' {
                        Intent::Quit
                    } else if c == '?' {
                        Intent::ToggleHelp
                    } else {
                        Intent::Ignore
                    }
                }
            }
        }
        _ => Intent::Ignore,
    }
}
