pub mod alerts_screen;
pub mod analytics;
pub mod dashboard;
pub mod device_details;
pub mod devices;
pub mod help;
pub mod settings;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;

pub fn placeholder(app: &App, frame: &mut Frame, area: Rect, text: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(text, Style::default().fg(app.theme.warning()))),
            Line::from(Span::styled(
                "this screen is not implemented in this build",
                Style::default().fg(app.theme.secondary()),
            )),
        ]),
        inner,
    );
}

pub fn section(app: &App, frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>) {
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Wrap, so a note longer than the panel is read in full instead of clipped.
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn kv(app: &App, key: &str, value: String, colour_bold: bool) -> Line<'static> {
    let theme = &app.theme;
    Line::from(vec![
        Span::styled(format!("{key} "), Style::default().fg(theme.secondary())),
        Span::styled(
            value,
            Style::default()
                .fg(if colour_bold {
                    theme.accent()
                } else {
                    theme.text()
                })
                .add_modifier(if colour_bold {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ])
}
