use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::app::App;
use crate::services::Level;

/// Alert history, newest at the top.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let block = Block::default()
        .title(format!(" ALERTS {} ", app.alerts.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.alerts.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no alerts",
                Style::default().fg(theme.secondary()),
            )),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .alerts
        .iter()
        .rev()
        .take(200)
        .map(|a| {
            let colour = match a.level {
                Level::Critical => theme.critical(),
                Level::Warn => theme.warning(),
                Level::Info => theme.secondary(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    a.at.format("%H:%M:%S").to_string(),
                    Style::default().fg(theme.secondary()),
                ),
                Span::styled(format!(" {}", a.sn), Style::default().fg(theme.text())),
                Span::styled(
                    format!(" {}", a.text),
                    Style::default().fg(colour).add_modifier(Modifier::BOLD),
                ),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}
