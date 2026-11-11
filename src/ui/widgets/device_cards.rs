use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::utils::formatting::{percent, watts};

/// Compact cards for every device, so a multi-device account fits on one screen.
/// The selected card is marked, and the bar colour follows the alert thresholds.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if app.devices.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no devices",
                Style::default().fg(theme.secondary()),
            )),
            area,
        );
        return;
    }

    let visible = app.visible_devices();
    let cards = visible.len().min(4);
    let constraints = vec![Constraint::Fill(1); cards];
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for (i, idx) in visible.iter().take(cards).enumerate() {
        let d = &app.devices[*idx];
        let selected = *idx == app.selected;
        let name = format!(" {} ", d.product_name);
        let border = if selected {
            theme.border_focus()
        } else {
            theme.border()
        };
        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(
                    if selected { "▶ " } else { "  " },
                    Style::default().fg(theme.accent()),
                ),
                Span::styled(
                    name,
                    Style::default()
                        .fg(theme.text())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if d.online { " ●" } else { " ○" },
                    Style::default().fg(if d.online {
                        theme.online()
                    } else {
                        theme.offline()
                    }),
                ),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border));
        let inner = block.inner(chunks[i]);
        frame.render_widget(block, chunks[i]);

        let soc = d.battery.soc;
        let colour = soc
            .map(|s| {
                theme.battery_colour(
                    s,
                    app.config.alerts.battery_low_percent,
                    app.config.alerts.battery_critical_percent,
                )
            })
            .unwrap_or(theme.secondary());
        let bar_w = (inner.width.saturating_sub(6)).clamp(6, 20) as usize;
        let gauge = soc
            .map(|s| theme.bar(bar_w, s / 100.0))
            .unwrap_or_else(|| "--".to_string());

        let lines = vec![
            Line::from(vec![
                Span::styled(gauge, Style::default().fg(colour)),
                Span::styled(format!(" {}", percent(soc)), Style::default().fg(colour)),
            ]),
            Line::from(vec![
                Span::styled("IN ", Style::default().fg(theme.secondary())),
                Span::styled(
                    watts(d.power.total_in_w),
                    Style::default().fg(theme.accent()),
                ),
                Span::styled(" OUT ", Style::default().fg(theme.secondary())),
                Span::styled(
                    watts(d.power.total_out_w),
                    Style::default().fg(theme.warning()),
                ),
            ]),
            Line::from(Span::styled(
                format!(
                    "{} · {}",
                    d.masked_serial(),
                    if d.source.is_mock() {
                        "MOCK"
                    } else {
                        d.source.label()
                    }
                ),
                Style::default().fg(theme.secondary()),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
