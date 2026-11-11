use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Screen};
use crate::input::Action;

/// Tab strip with the configured keys, plus the keys that always work.
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
    let mut spans: Vec<Span> = Vec::new();
    for (screen, action) in tabs {
        let key = app.keys.key_for(action);
        let label = format!(" {} {} ", key, screen.label());
        let active = core::mem::discriminant(&screen) == current;
        spans.push(Span::styled(
            label,
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
    spans.push(Span::styled(
        format!(" {} Help ", app.keys.key_for(Action::Help)),
        Style::default().fg(theme.secondary()),
    ));
    spans.push(Span::styled(
        format!(" {} Quit ", app.keys.key_for(Action::Quit)),
        Style::default().fg(theme.secondary()),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
