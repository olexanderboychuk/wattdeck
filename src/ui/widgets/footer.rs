use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::app::{App, Screen};
use crate::input::Action;
use crate::ui::widgets::pad_right;

/// One compact navigation line: the tab strip on the left, the keys that always
/// work on the right, in fixed columns.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let tabs = [
        (Screen::Dashboard, Action::Dashboard),
        (Screen::Devices, Action::Devices),
        (Screen::Analytics, Action::Analytics),
        (Screen::Alerts, Action::Alerts),
        (Screen::Settings, Action::Settings),
    ];
    let current = core::mem::discriminant(&app.screen);
    let mut left: Vec<Span> = Vec::new();
    for (screen, action) in tabs {
        let key = app.keys.key_for(action);
        let active = core::mem::discriminant(&screen) == current;
        left.push(Span::styled(
            format!(" {} {} ", key, screen.label()),
            Style::default()
                .fg(if active {
                    theme.accent()
                } else {
                    theme.secondary()
                })
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
    }
    let right = vec![Span::styled(
        format!(
            "{} device · {} help · {} quit ",
            app.keys.key_for(Action::Up),
            app.keys.key_for(Action::Help),
            app.keys.key_for(Action::Quit),
        ),
        Style::default().fg(theme.secondary()),
    )];
    let line = pad_right(left, right, area.width);
    frame.render_widget(Paragraph::new(line), area);
}
