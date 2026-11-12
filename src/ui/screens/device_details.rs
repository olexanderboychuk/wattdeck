use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::telemetry::models::DeviceSnapshot;
use crate::ui::layout::Areas;
use crate::ui::screens::{kv, section};
use crate::ui::snapshot::Snapshot;
use crate::ui::widgets::device_cards::state_colour;
use crate::ui::widgets::{self, battery::raw_rows};
use crate::utils::formatting::{elapsed, pad};

pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, areas: Areas) {
    let Some(d) = app.selected_device() else {
        crate::ui::screens::placeholder(app, frame, areas.main, "select a device");
        return;
    };
    let theme = &app.theme;

    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    crate::ui::widgets::battery::draw(app, snap, frame, left);

    // Controls: the verified commands for this product, gated by the capabilities
    // the device reports and by the config's `[controls] enabled`. The value the
    // device reported is shown next to each control, so the row says what the
    // device says it is set to, and what the keys would change it to.
    let rows = app.control_rows();
    let mut control_lines: Vec<Line<'static>> = Vec::new();
    if rows.is_empty() {
        control_lines.push(Line::from(Span::styled(
            if app.config.controls.enabled {
                "no verified commands for this product"
            } else {
                "controls are off: set [controls] enabled = true"
            },
            Style::default().fg(theme.warning()),
        )));
    }
    for (label, reported, candidate, selected) in rows {
        let mut spans = vec![
            Span::styled(pad(&label, 17), Style::default().fg(theme.secondary())),
            Span::styled(
                format!("reported {}", pad(&reported, 9)),
                Style::default().fg(theme.text()),
            ),
        ];
        if !candidate.is_empty() && candidate != "no change" {
            spans.push(Span::styled(
                format!("-> {candidate}"),
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if selected {
            spans.push(Span::styled("  <-", Style::default().fg(theme.accent())));
        }
        control_lines.push(Line::from(spans));
    }
    control_lines.push(Line::from(Span::styled(
        "c/C = control, left/right = value, enter = confirm",
        Style::default().fg(theme.secondary()),
    )));
    if app.pending_control.is_some() {
        control_lines.push(Line::from(Span::styled(
            "waiting for confirmation",
            Style::default()
                .fg(theme.warning())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if right.width > 0 {
        let split = ratatui::layout::Layout::new(
            ratatui::layout::Direction::Vertical,
            [Constraint::Min(6), Constraint::Min(3), Constraint::Fill(1)],
        )
        .split(right);
        // The diagnostics row is what the device screen is for, so it gets the room
        // first; the raw inspector is the developer view and is last.
        section(app, frame, split[0], "DEVICE", status_lines(app, d, snap));
        section(app, frame, split[1], "CONTROLS", control_lines);
        let rows = raw_rows(d, "", (split[2].height.saturating_sub(2)) as usize);
        let lines: Vec<ratatui::text::Line> = rows
            .into_iter()
            .take(12)
            .map(|(k, v)| kv(app, &k, v, false))
            .collect();
        section(app, frame, split[2], "RAW INSPECTOR", lines);
    }

    widgets::power_graph::history(app, snap, frame, areas.history);
}

/// Per-device diagnostics: the state the app can defend, when each transport last
/// answered, how many properties the device has sent, and what the model is known
/// to support. Two lines, because the details column is short.
fn status_lines(app: &App, d: &DeviceSnapshot, snap: &Snapshot) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let state = d.state(app.now, app.stale_window_secs());
    let caps = snap.caps.summary();
    let mut lines = vec![Line::from(vec![
        Span::styled("state ", Style::default().fg(theme.secondary())),
        Span::styled(
            state.label(),
            Style::default().fg(state_colour(state, theme)),
        ),
        Span::styled("  live data ", Style::default().fg(theme.secondary())),
        Span::styled(ago(d.last_mqtt, app.now), Style::default().fg(theme.text())),
        Span::styled("  cloud data ", Style::default().fg(theme.secondary())),
        Span::styled(ago(d.last_rest, app.now), Style::default().fg(theme.text())),
    ])];
    lines.push(Line::from(vec![
        Span::styled("measurements ", Style::default().fg(theme.secondary())),
        Span::styled(d.raw.len().to_string(), Style::default().fg(theme.text())),
        Span::styled("  productType ", Style::default().fg(theme.secondary())),
        Span::styled(
            d.product_type.to_string(),
            Style::default().fg(theme.text()),
        ),
        Span::styled("  supports ", Style::default().fg(theme.secondary())),
        Span::styled(
            if caps.is_empty() {
                "nothing verified".to_string()
            } else {
                caps.join(", ")
            },
            Style::default().fg(theme.text()),
        ),
    ]));
    if let Some(refusal) = &d.last_refusal {
        lines.push(Line::from(vec![
            Span::styled("cloud says ", Style::default().fg(theme.warning())),
            Span::styled(refusal.clone(), Style::default().fg(theme.warning())),
        ]));
    }
    lines
}

/// "3 s ago", or "none yet" when that transport has never answered.
fn ago(t: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    match t {
        Some(t) => elapsed((now - t).num_seconds() as f64),
        None => "none yet".to_string(),
    }
}
