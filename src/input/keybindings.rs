use std::collections::HashMap;

use crossterm::event::KeyEvent;

use crate::input::{Action, parse_key};

/// Action name -> key binding. Loaded from the `[keys]` section of the config,
/// so a user can remap everything.
#[derive(Debug, Clone)]
pub struct KeyMap {
    bindings: Vec<(Action, KeyEvent)>,
}

impl Default for KeyMap {
    fn default() -> Self {
        let mut bindings: Vec<(Action, KeyEvent)> = Vec::new();
        let defaults: [(Action, &str); 17] = [
            (Action::Dashboard, "1"),
            (Action::Devices, "2"),
            (Action::Analytics, "3"),
            (Action::Alerts, "4"),
            (Action::Settings, "5"),
            (Action::Help, "?"),
            (Action::NextScreen, "tab"),
            (Action::PrevScreen, "shift+tab"),
            (Action::NextPanel, "l"),
            (Action::PrevPanel, "h"),
            (Action::Up, "up"),
            (Action::Down, "down"),
            (Action::Left, "left"),
            (Action::Right, "right"),
            (Action::Enter, "enter"),
            (Action::Escape, "esc"),
            (Action::Refresh, "r"),
        ];
        for (action, spec) in defaults {
            bindings.push((action, parse_key(spec).expect("default binding parses")));
        }
        for (action, spec) in [
            (Action::Quit, "q"),
            (Action::CycleScenario, "s"),
            (Action::ToggleSerial, "i"),
            (Action::ToggleTempUnit, "u"),
            (Action::CycleFps, "f"),
            (Action::NextControl, "c"),
            (Action::PrevControl, "shift+c"),
            (Action::Confirm, "y"),
            (Action::Cancel, "n"),
            (Action::PageUp, "pgup"),
            (Action::PageDown, "pgdn"),
            (Action::Home, "home"),
            (Action::End, "end"),
        ] {
            bindings.push((action, parse_key(spec).expect("default binding parses")));
        }
        Self { bindings }
    }
}

impl KeyMap {
    /// Build from the config's `[keys]` table, e.g. `quit = "ctrl+q"`.
    pub fn from_config(overrides: &HashMap<String, String>) -> Self {
        let mut map = KeyMap::default();
        for (action, key) in overrides {
            match map.override_binding(action, key) {
                Ok(()) => {}
                Err(e) => tracing::warn!("ignoring keybinding {action} = {key}: {e}"),
            }
        }
        map
    }

    pub fn action_for(&self, event: KeyEvent) -> Option<Action> {
        // A binding is unique, so a linear scan over 23 entries is cheaper than
        // maintaining a reverse index.
        self.bindings
            .iter()
            .find(|(_, k)| *k == event)
            .map(|(a, _)| *a)
    }

    /// Remap one action. Returns an error for an unknown action name or an
    /// unparsable key, so a typo in the config cannot silently disable a key.
    pub fn override_binding(&mut self, action: &str, key: &str) -> anyhow::Result<()> {
        let action = action_from_name(action)?;
        let event = parse_key(key).ok_or_else(|| anyhow::anyhow!("cannot parse key '{key}'"))?;
        // Reject collisions: one key cannot trigger two actions.
        if let Some(existing) = self
            .bindings
            .iter()
            .find(|(a, k)| *k == event && *a != action)
        {
            anyhow::bail!(
                "key '{key}' is already bound to {}",
                existing.0.default_key()
            );
        }
        self.bindings.retain(|(a, _)| *a != action);
        self.bindings.push((action, event));
        Ok(())
    }

    pub fn key_for(&self, action: Action) -> String {
        let event = self
            .bindings
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, k)| *k);
        match event {
            Some(k) => key_name(&k),
            None => action.default_key().to_string(),
        }
    }
}

pub fn key_name(event: &KeyEvent) -> String {
    let mut out = String::new();
    // BackTab already carries the shift in its name.
    if event.code != crossterm::event::KeyCode::BackTab
        && event
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT)
    {
        out.push_str("shift+");
    }
    if event
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        out.push_str("ctrl+");
    }
    if event
        .modifiers
        .contains(crossterm::event::KeyModifiers::ALT)
    {
        out.push_str("alt+");
    }
    match event.code {
        crossterm::event::KeyCode::Char(c) => out.push(c),
        crossterm::event::KeyCode::Enter => out.push_str("enter"),
        crossterm::event::KeyCode::Esc => out.push_str("esc"),
        crossterm::event::KeyCode::Tab => out.push_str("tab"),
        crossterm::event::KeyCode::BackTab => out.push_str("shift+tab"),
        crossterm::event::KeyCode::Backspace => out.push_str("backspace"),
        crossterm::event::KeyCode::PageUp => out.push_str("pgup"),
        crossterm::event::KeyCode::PageDown => out.push_str("pgdn"),
        crossterm::event::KeyCode::Home => out.push_str("home"),
        crossterm::event::KeyCode::End => out.push_str("end"),
        crossterm::event::KeyCode::Up => out.push_str("up"),
        crossterm::event::KeyCode::Down => out.push_str("down"),
        crossterm::event::KeyCode::Left => out.push_str("left"),
        crossterm::event::KeyCode::Right => out.push_str("right"),
        crossterm::event::KeyCode::F(n) => out.push_str(&format!("f{n}")),
        _ => out.push('?'),
    }
    out
}

fn action_from_name(name: &str) -> anyhow::Result<Action> {
    let table: Vec<(Action, &str)> = vec![
        (Action::Dashboard, "dashboard"),
        (Action::Devices, "devices"),
        (Action::Analytics, "analytics"),
        (Action::Alerts, "alerts"),
        (Action::Settings, "settings"),
        (Action::Help, "help"),
        (Action::NextScreen, "next_screen"),
        (Action::PrevScreen, "prev_screen"),
        (Action::NextPanel, "next_panel"),
        (Action::PrevPanel, "prev_panel"),
        (Action::Up, "up"),
        (Action::Down, "down"),
        (Action::Left, "left"),
        (Action::Right, "right"),
        (Action::Enter, "enter"),
        (Action::Escape, "escape"),
        (Action::Refresh, "refresh"),
        (Action::Quit, "quit"),
        (Action::CycleScenario, "cycle_scenario"),
        (Action::ToggleSerial, "toggle_serial"),
        (Action::ToggleTempUnit, "toggle_temp_unit"),
        (Action::CycleFps, "cycle_fps"),
        (Action::PageUp, "page_up"),
        (Action::PageDown, "page_down"),
        (Action::Home, "home"),
        (Action::End, "end"),
    ];
    table
        .into_iter()
        .find(|(_, n)| *n == name)
        .map(|(a, _)| a)
        .ok_or_else(|| anyhow::anyhow!("unknown action '{name}'"))
}
