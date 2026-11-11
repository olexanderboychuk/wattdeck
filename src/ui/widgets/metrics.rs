use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::ui::widgets::battery::raw_rows;

/// Raw property inspector: the dotted names the device reported, verbatim. Used in
/// the device detail screen so a user can see exactly what the device sends,
/// including keys the app does not understand.
pub fn draw(app: &App, frame: &mut Frame, area: Rect, filter: &str) {
    let theme = &app.theme;
    let Some(d) = app.selected_device() else {
        return;
    };
    let block = Block::default()
        .title(format!(
            " RAW PROPERTIES {} ({} keys) ",
            if filter.is_empty() {
                String::new()
            } else {
                format!("[{filter}]")
            },
            d.raw.len()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let limit = inner.height.saturating_sub(1) as usize;
    let rows = raw_rows(d, filter, limit);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no properties",
                Style::default().fg(theme.secondary()),
            )),
            inner,
        );
        return;
    }
    let width = inner.width as usize;
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, v)| {
            let text = format!("{k} = {v}");
            let text: String = text.chars().take(width).collect();
            Line::from(Span::styled(text, Style::default().fg(theme.text())))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The same rows, used by tests and by the `export` command.
pub fn rows(app: &App, filter: &str, limit: usize) -> Vec<(String, String)> {
    match app.selected_device() {
        Some(d) => raw_rows(d, filter, limit),
        None => Vec::new(),
    }
}
