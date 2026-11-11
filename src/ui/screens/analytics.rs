use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, RANGES};
use crate::telemetry::aggregator::{
    bucket_label, bucket_width_for, bucketize, cost_wh, width_name,
};
use crate::telemetry::history::{integrate_energy, mean_power, peak_powers};
use crate::ui::layout::Areas;
use crate::ui::screens::{kv, section};

/// A gap longer than 15 minutes is a device that stopped reporting, not energy.
const GAP_SECS: i64 = 900;
use crate::ui::widgets;
use crate::utils::formatting::{grouped, pad, plural, watts, wh};

pub fn draw(app: &App, frame: &mut Frame, areas: Areas) {
    let theme = &app.theme;
    let Some(d) = app.selected_device() else {
        crate::ui::screens::placeholder(app, frame, areas.main, "select a device");
        return;
    };

    // Range selector.
    let mut spans: Vec<Span> = Vec::new();
    for (i, (label, _)) in RANGES.iter().enumerate() {
        let active = i == app.range;
        spans.push(Span::styled(
            format!(" {label} "),
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
    frame.render_widget(Paragraph::new(Line::from(spans)), areas.strip);

    let samples = app.range_samples(&d.sn);
    let covered = app.range_covered(&d.sn);
    let (peak_in, peak_out) = peak_powers(&samples);
    let mean_in = mean_power(&samples, |s| s.in_w);
    let mean_out = mean_power(&samples, |s| s.out_w);
    let energy_in = integrate_energy(&samples, |s| s.in_w);
    let energy_out = integrate_energy(&samples, |s| s.out_w);
    let solar = integrate_energy(&samples, |s| s.solar_w);

    let mut stats = vec![
        kv(app, "samples", grouped(samples.len() as i64), false),
        kv(app, "energy in", wh(Some(energy_in)), true),
        kv(app, "energy out", wh(Some(energy_out)), false),
        kv(app, "solar energy", wh(Some(solar)), true),
        kv(app, "peak in", watts(Some(peak_in)), false),
        kv(app, "peak out", watts(Some(peak_out)), false),
        kv(app, "mean in", watts(Some(mean_in)), false),
        kv(app, "mean out", watts(Some(mean_out)), false),
    ];
    // Cost is the user's own tariff, so the line appears only when it is set, and
    // says which tariff it used.
    let tariff = app.config.analytics.tariff_per_kwh;
    if tariff > 0.0 {
        stats.push(kv(
            app,
            "cost in",
            format!("{:.2} at {:.2} per kWh", cost_wh(energy_in, tariff), tariff),
            true,
        ));
        stats.push(kv(
            app,
            "cost out",
            format!(
                "{:.2} at {:.2} per kWh",
                cost_wh(energy_out, tariff),
                tariff
            ),
            false,
        ));
    }

    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    section(
        app,
        frame,
        left,
        &format!(" ANALYTICS · {} ", RANGES[app.range].0),
        stats,
    );

    // Bucketed totals at the width that suits the range, newest first. A 15-minute
    // range gets per-minute rows, a 30-day range gets weekly rows, so the table
    // always has the resolution the samples can support.
    let width = bucket_width_for(RANGES[app.range].1);
    let buckets = bucketize(&samples, width, GAP_SECS);
    let mut bucket_lines: Vec<Line> = Vec::new();
    for b in buckets.iter().rev().take(10) {
        bucket_lines.push(Line::from(vec![
            Span::styled(
                pad(&bucket_label(&b.start, width), 8),
                Style::default().fg(theme.secondary()),
            ),
            Span::styled(
                format!(
                    "in {}  out {}  solar {}  peak {} / {}",
                    wh(Some(b.energy_in_wh)),
                    wh(Some(b.energy_out_wh)),
                    wh(Some(b.solar_wh)),
                    watts(Some(b.peak_in_w)),
                    watts(Some(b.peak_out_w))
                ),
                Style::default().fg(theme.text()),
            ),
        ]));
    }

    let mut notes: Vec<Line> = Vec::new();
    if samples.len() < 2 {
        notes.push(Line::from(Span::styled(
            format!(
                "insufficient data: {} in this range, 2 are needed to compute energy",
                plural(samples.len(), "sample")
            ),
            Style::default().fg(theme.warning()),
        )));
    }
    // Rows that came from the database are worth naming: a long range is not the
    // live window, and the user should know where the numbers come from.
    let rows = app.db_rows(&d.sn);
    if rows > 0 {
        notes.push(Line::from(Span::styled(
            format!(
                "history database supplied {} in this range",
                plural(rows, "sample")
            ),
            Style::default().fg(theme.accent()),
        )));
    }
    if !covered {
        notes.push(Line::from(Span::styled(
            format!(
                "partial coverage: {} {} less than the requested {}",
                plural(samples.len(), "sample"),
                if samples.len() == 1 { "spans" } else { "span" },
                RANGES[app.range].0.to_lowercase()
            ),
            Style::default().fg(theme.warning()),
        )));
        // Say what the app can do about it, and what stops it from doing that.
        let window = app.window_minutes();
        if app.mock {
            notes.push(Line::from(Span::styled(
                "mock run: the demo is not recorded, so no range can reach past the memory window",
                Style::default().fg(theme.warning()),
            )));
        } else if app.history_stored() {
            notes.push(Line::from(Span::styled(
                format!(
                    "the memory window holds about {window} min; longer ranges are read from the history database as samples arrive",
                    ),
                Style::default().fg(theme.secondary()),
            )));
        } else {
            notes.push(Line::from(Span::styled(
                "history database is off: set [history] enabled = true to record beyond the memory window",
                Style::default().fg(theme.warning()),
            )));
        }
    }
    notes.push(Line::from(Span::styled(
        "energy is integrated from power over time; gaps longer than 15 min are not counted",
        Style::default().fg(theme.secondary()),
    )));

    if right.width > 0 {
        let (top, bottom) =
            crate::ui::layout::rows(right, Constraint::Fill(1), Constraint::Fill(1));
        section(
            app,
            frame,
            top,
            &format!(" BUCKETS · {} ", width_name(width)),
            bucket_lines,
        );
        section(app, frame, bottom, "DATA QUALITY", notes);
    } else {
        section(app, frame, areas.history, "DATA QUALITY", notes);
    }
    // The chart band is shared with the dashboard so the same history is visible in
    // both views.
    widgets::power_graph::history(app, frame, areas.history);
}
