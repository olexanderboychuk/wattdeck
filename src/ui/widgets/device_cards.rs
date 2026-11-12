use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::telemetry::models::DeviceState;
use crate::ui::snapshot::Snapshot;
use crate::ui::theme::Theme;
use crate::utils::formatting::{pad, percent, watts};

/// The device list: one row per device, so a multi-device account fits on one
/// screen and the selected device is obvious. One row is one device, so the band
/// never grows whitespace.
pub fn draw(app: &App, _snap: &Snapshot, frame: &mut Frame, area: Rect) {
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
    if area.height < 3 || area.width < 12 {
        return;
    }
    let block = Block::default()
        .title(" DEVICES ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = app.visible_devices();
    let show_serial = inner.width >= 46;
    let show_source = inner.width >= 62;
    let mut rows: Vec<Line> = Vec::new();
    for idx in visible.iter().take(inner.height as usize) {
        let d = &app.devices[*idx];
        let selected = *idx == app.selected;
        let soc = d.battery.soc;
        let state = d.state(app.now, app.stale_window_secs());
        let colour = soc
            .map(|s| {
                theme.battery_colour(
                    s,
                    app.config.alerts.battery_low_percent,
                    app.config.alerts.battery_critical_percent,
                )
            })
            .unwrap_or(theme.secondary());
        let mut spans = vec![
            Span::styled(
                if selected { "▶ " } else { "  " },
                Style::default().fg(theme.accent()),
            ),
            Span::styled(
                pad(&d.product_name, 13),
                Style::default()
                    .fg(if d.online {
                        theme.text()
                    } else {
                        theme.offline()
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(" ", Style::default().fg(theme.secondary())),
            Span::styled(
                pad(state.label(), 13),
                Style::default().fg(state_colour(state, theme)),
            ),
            Span::styled(" ", Style::default().fg(theme.secondary())),
            Span::styled(
                format!("{:>5} ", percent(soc)),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "IN {:>7} OUT {:>7} ",
                    watts(d.power.total_in_w),
                    watts(d.power.total_out_w)
                ),
                Style::default().fg(theme.text()),
            ),
        ];
        if show_serial {
            spans.push(Span::styled(
                format!("{} ", d.masked_serial()),
                Style::default().fg(theme.secondary()),
            ));
        }
        if show_source {
            spans.push(Span::styled(
                d.source.label().to_string(),
                Style::default().fg(if d.source.is_mock() {
                    theme.mock()
                } else {
                    theme.secondary()
                }),
            ));
        }
        rows.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(rows), inner);
}

/// The state's colour: green when data is arriving, yellow when it has gone stale,
/// red when the cloud refused, gray when there is nothing to report.
pub fn state_colour(state: DeviceState, theme: &Theme) -> Color {
    match state {
        DeviceState::Online => theme.online(),
        DeviceState::Stale => theme.warning(),
        DeviceState::Connecting => theme.secondary(),
        DeviceState::AuthError => theme.critical(),
        DeviceState::Offline | DeviceState::Unsupported => theme.offline(),
        DeviceState::Unknown => theme.secondary(),
    }
}
