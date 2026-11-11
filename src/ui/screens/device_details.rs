use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::screens::{kv, section};
use crate::ui::widgets::{self, battery::raw_rows};
use crate::utils::formatting::pad;

pub fn draw(app: &App, frame: &mut Frame, areas: Areas) {
    let Some(d) = app.selected_device() else {
        crate::ui::screens::placeholder(app, frame, areas.main, "select a device");
        return;
    };
    let theme = &app.theme;

    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    crate::ui::widgets::battery::draw(app, frame, left);

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
        let (top, bottom) =
            crate::ui::layout::rows(right, Constraint::Fill(2), Constraint::Fill(1));
        section(app, frame, top, "CONTROLS", control_lines);
        let rows = raw_rows(d, "", (bottom.height.saturating_sub(2)) as usize);
        let lines: Vec<ratatui::text::Line> = rows
            .into_iter()
            .take(12)
            .map(|(k, v)| kv(app, &k, v, false))
            .collect();
        section(app, frame, bottom, "RAW INSPECTOR", lines);
    }

    widgets::power_graph::history(app, frame, areas.history);
}
