use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::input::Action;
use crate::ui::layout::Areas;
use crate::ui::screens::section;
use crate::ui::snapshot::Snapshot;

const HELP_ROWS: [(Action, &str); 20] = [
    (Action::Dashboard, "dashboard"),
    (Action::Devices, "device list"),
    (Action::Analytics, "analytics"),
    (Action::Alerts, "alerts"),
    (Action::Settings, "settings"),
    (Action::NextScreen, "next screen"),
    (Action::PrevScreen, "previous screen"),
    (Action::Up, "previous device"),
    (Action::Down, "next device"),
    (Action::Left, "previous panel / value"),
    (Action::Right, "next panel / value"),
    (Action::NextControl, "next control"),
    (Action::PrevControl, "previous control"),
    (Action::Confirm, "send the shown command"),
    (Action::Cancel, "cancel the command"),
    (Action::Refresh, "request refresh"),
    (Action::ToggleSerial, "show raw serial"),
    (Action::ToggleTempUnit, "temperature unit"),
    (Action::CycleFps, "frame rate"),
    (Action::Quit, "quit"),
];

pub fn draw(app: &App, _snap: &Snapshot, frame: &mut Frame, areas: Areas) {
    let theme = &app.theme;
    let rows: Vec<Line> = HELP_ROWS
        .iter()
        .map(|(action, what)| {
            let key = app.keys.key_for(*action);
            Line::from(vec![
                Span::styled(
                    format!("{key:<10}"),
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*what, Style::default().fg(theme.text())),
            ])
        })
        .collect();

    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    section(app, frame, left, "KEYS", rows);

    let mut notes: Vec<Line> = Vec::new();
    notes.push(Line::from(Span::styled(
        "Keys are configurable in the [keys] section of config.toml.",
        Style::default().fg(theme.secondary()),
    )));
    notes.push(Line::from(Span::styled(
        "Navigation keys are read-only. A device command needs [controls] enabled = true, \
         a control picked with c, a value set with left/right, and a confirmation with enter/y; \
         n/esc cancels and sends nothing.",
        Style::default().fg(theme.charging()),
    )));
    notes.push(Line::from(Span::styled(
        format!(
            "Scenario: {} (press {} to switch, mock mode only)",
            app.scenario.label(),
            app.keys.key_for(Action::CycleScenario)
        ),
        Style::default().fg(theme.mock()),
    )));
    notes.push(Line::from(Span::styled(
        format!(
            "Serial display: {} · colour mode: {:?} · fps: {}",
            if app.config.ui.show_serial {
                "raw"
            } else {
                "masked"
            },
            app.theme.mode,
            app.config.ui.fps
        ),
        Style::default().fg(theme.secondary()),
    )));
    notes.push(Line::from(Span::styled(
        "Controls are disabled in this build (read-only mode).",
        Style::default().fg(theme.secondary()),
    )));
    if right.width > 0 {
        section(app, frame, right, "NOTES", notes);
    }
    let _ = Constraint::Min(1);
}
