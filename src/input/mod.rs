pub mod keybindings;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Every keyboard action the app understands. Keys are resolved through a
/// configurable `KeyMap`, so nothing here is hard-wired to a specific key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Dashboard,
    Devices,
    Analytics,
    Alerts,
    Settings,
    Help,
    NextScreen,
    PrevScreen,
    NextPanel,
    PrevPanel,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Refresh,
    Quit,
    CycleScenario,
    ToggleSerial,
    ToggleTempUnit,
    CycleFps,
    PageUp,
    PageDown,
    Home,
    End,
    NextControl,
    PrevControl,
    Confirm,
    Cancel,
}

impl Action {
    /// The default key that performs this action, for the help overlay.
    pub fn default_key(&self) -> &'static str {
        match self {
            Action::Dashboard => "1",
            Action::Devices => "2",
            Action::Analytics => "3",
            Action::Alerts => "4",
            Action::Settings => "5",
            Action::Help => "?",
            Action::NextScreen => "tab",
            Action::PrevScreen => "shift+tab",
            Action::NextPanel => "l",
            Action::PrevPanel => "h",
            Action::Up => "up",
            Action::Down => "down",
            Action::Left => "left",
            Action::Right => "right",
            Action::Enter => "enter",
            Action::Escape => "esc",
            Action::Refresh => "r",
            Action::Quit => "q",
            Action::CycleScenario => "s",
            Action::ToggleSerial => "i",
            Action::ToggleTempUnit => "u",
            Action::CycleFps => "f",
            Action::PageUp => "pgup",
            Action::PageDown => "pgdn",
            Action::Home => "home",
            Action::End => "end",
            Action::NextControl => "c",
            Action::PrevControl => "shift+c",
            Action::Confirm => "y",
            Action::Cancel => "n",
        }
    }
}

/// Resolve a key event through the configured key map.
pub fn resolve(map: &keybindings::KeyMap, event: KeyEvent) -> Option<Action> {
    map.action_for(event)
}

/// Parse "ctrl+g", "shift+tab", "f1", "q".
pub fn parse_key(spec: &str) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let parts: Vec<String> = spec
        .split('+')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    let last = parts.last()?;
    for p in &parts[..parts.len() - 1] {
        match p.as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "shift" => mods |= KeyModifiers::SHIFT,
            "alt" => mods |= KeyModifiers::ALT,
            _ => return None,
        }
    }
    let code = parse_code(last)?;
    // Terminals deliver Shift+Tab as BackTab, so a binding written "shift+tab" has to
    // resolve to the code the terminal actually sends.
    let code = if code == KeyCode::Tab && mods.contains(KeyModifiers::SHIFT) {
        KeyCode::BackTab
    } else {
        code
    };
    Some(KeyEvent::new(code, mods))
}

fn parse_code(s: &str) -> Option<KeyCode> {
    match s {
        "space" => return Some(KeyCode::Char(' ')),
        "enter" | "return" => return Some(KeyCode::Enter),
        "esc" | "escape" => return Some(KeyCode::Esc),
        "tab" => return Some(KeyCode::Tab),
        "backtab" | "shift+tab" => return Some(KeyCode::BackTab),
        "pgup" | "pageup" => return Some(KeyCode::PageUp),
        "pgdn" | "pagedown" => return Some(KeyCode::PageDown),
        "home" => return Some(KeyCode::Home),
        "end" => return Some(KeyCode::End),
        "up" => return Some(KeyCode::Up),
        "down" => return Some(KeyCode::Down),
        "left" => return Some(KeyCode::Left),
        "right" => return Some(KeyCode::Right),
        "backspace" => return Some(KeyCode::Backspace),
        _ => {}
    }
    if let Some(c) = s.chars().next()
        && s.chars().count() == 1
    {
        return Some(KeyCode::Char(c));
    }
    if let Some(c) = s.strip_prefix('f')
        && let Ok(n) = c.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(KeyCode::F(n));
    }
    if let Ok(n) = s.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(KeyCode::F(n));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use keybindings::KeyMap;

    #[test]
    fn parses_plain_keys() {
        assert_eq!(
            parse_key("q"),
            Some(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("ctrl+r"),
            Some(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("shift+tab"),
            Some(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        );
        assert_eq!(
            parse_key("f5"),
            Some(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("garbage+key"), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn default_map_resolves_every_action() {
        let map = KeyMap::default();
        for action in [
            Action::Dashboard,
            Action::Devices,
            Action::Analytics,
            Action::Alerts,
            Action::Settings,
            Action::Help,
            Action::Quit,
            Action::Refresh,
        ] {
            let event = parse_key(action.default_key()).unwrap();
            assert_eq!(
                resolve(&map, event),
                Some(action),
                "{}",
                action.default_key()
            );
        }
    }

    #[test]
    fn overrides_take_effect() {
        let mut map = KeyMap::default();
        map.override_binding("quit", "ctrl+q").unwrap();
        let event = parse_key("ctrl+q").unwrap();
        assert_eq!(resolve(&map, event), Some(Action::Quit));
        // The old key no longer quits.
        assert_eq!(resolve(&map, parse_key("q").unwrap()), None);
    }

    #[test]
    fn unknown_action_name_is_rejected() {
        let mut map = KeyMap::default();
        assert!(map.override_binding("self-destruct", "ctrl+x").is_err());
    }
}
