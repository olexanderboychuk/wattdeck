use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::services::{ConnState, TransportInfo};
use crate::ui::snapshot::Snapshot;
use crate::ui::theme::Theme;
use crate::ui::widgets::pad_right;
use crate::utils::formatting::elapsed;

/// The connection panel: what each path last did, in plain words. A device the cloud
/// refused is not reported here, because the cloud answered that request - the refusal
/// belongs to the device panel.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if area.height < 5 || area.width < 28 {
        return;
    }
    let block = Block::default()
        .title(" CONNECTION ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut rows: Vec<Line> = Vec::new();
    for (label, info) in [
        ("cloud", app.connection.rest_info()),
        ("live data", app.connection.mqtt_info()),
    ] {
        rows.push(state_line(label, info, theme, inner.width, app.now));
        rows.push(detail_line(info, theme, app.now));
    }
    rows.push(fresh_line(snap, theme, inner.width, app.now));

    rows.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(rows), inner);
}

/// The state and the last good answer, on one line.
fn state_line(
    label: &str,
    info: &TransportInfo,
    theme: &Theme,
    width: u16,
    now: DateTime<Utc>,
) -> Line<'static> {
    let left = vec![
        Span::styled(format!("{label} "), Style::default().fg(theme.secondary())),
        Span::styled(
            info.state.label(),
            Style::default()
                .fg(state_colour(info.state, theme))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let right = vec![Span::styled(
        match info.last_ok {
            Some(t) => format!("last good {} ago", elapsed((now - t).num_seconds() as f64)),
            None => "no request yet".to_string(),
        },
        Style::default().fg(theme.secondary()),
    )];
    pad_right(left, right, width)
}

/// What went wrong: the code the server answered with, how many failures in a row, and
/// the reconnect count. A success resets the failure count, so this row says whether
/// the path is currently bad or was bad earlier.
fn detail_line(info: &TransportInfo, theme: &Theme, now: DateTime<Utc>) -> Line<'static> {
    let text = match (info.status, info.fails, info.last_fail) {
        (Some(code), n, Some(t)) if n > 0 => format!(
            "code {code}, {n} in a row, {} ago",
            elapsed((now - t).num_seconds() as f64)
        ),
        (None, n, Some(t)) if n > 0 => format!(
            "{n} in a row, {} ago",
            elapsed((now - t).num_seconds() as f64)
        ),
        _ => format!("reconnects {}", info.reconnects),
    };
    Line::from(vec![Span::styled(
        text,
        Style::default().fg(if info.fails > 0 {
            theme.warning()
        } else {
            theme.secondary()
        }),
    )])
}

/// Telemetry freshness: a path can be online while the device stays silent, and that
/// is what monitoring depends on.
fn fresh_line(snap: &Snapshot, theme: &Theme, width: u16, now: DateTime<Utc>) -> Line<'static> {
    let left = vec![Span::styled(
        "device data ",
        Style::default().fg(theme.secondary()),
    )];
    let right = vec![Span::styled(
        match snap.device.map(|d| d.seconds_since_update(now)) {
            Some(secs) => format!("last seen {} ago", elapsed(secs)),
            None => "no device selected".to_string(),
        },
        Style::default().fg(if snap.device.map(|d| d.online).unwrap_or(false) {
            theme.secondary()
        } else {
            theme.warning()
        }),
    )];
    pad_right(left, right, width)
}

fn state_colour(state: ConnState, theme: &Theme) -> ratatui::style::Color {
    match state {
        ConnState::Connected => theme.charging(),
        ConnState::Initializing | ConnState::Connecting => theme.secondary(),
        ConnState::Degraded | ConnState::Reconnecting => theme.warning(),
        ConnState::Offline | ConnState::AuthError => theme.critical(),
    }
}
