use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};

use crate::app::App;
use crate::telemetry::models::{DeviceSnapshot, Provenance};
use crate::telemetry::runtime::{State, estimate, is_sentinel};
use crate::ui::snapshot::Snapshot;
use crate::utils::formatting::{duration_min, grouped, percent, temp, wh};

/// Device overview: the headline SOC, the gauge, the energy figures and the state
/// the device reports. Fields the device never sent are shown as `--`, never as a
/// fabricated zero, and every number says where it came from.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // A bordered panel needs at least one row of content; drawing in a smaller area
    // writes outside the buffer.
    if area.height < 3 || area.width < 6 {
        return;
    }
    let Some(d) = snap.device else {
        frame.render_widget(
            Paragraph::new("no device selected").style(Style::default().fg(theme.secondary())),
            area,
        );
        return;
    };
    let title = format!(" {} {} ", d.product_name, d.masked_serial());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let tight = inner.width < 46;

    let b = &d.battery;
    let soc = b.soc;
    let colour = soc
        .map(|s| {
            theme.battery_colour(
                s,
                app.config.alerts.battery_low_percent,
                app.config.alerts.battery_critical_percent,
            )
        })
        .unwrap_or(theme.secondary());

    // Headline: the SOC, then the gauge, both in the battery colour.
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} ", percent(soc)),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if tight { "LCD " } else { "battery " },
                Style::default().fg(theme.secondary()),
            ),
            Span::styled(
                b.soc_display
                    .map(|s| {
                        if tight {
                            percent(Some(s)).to_string()
                        } else {
                            format!("on the device LCD {}", percent(Some(s)))
                        }
                    })
                    .unwrap_or_else(|| "no LCD reading".to_string()),
                Style::default().fg(theme.secondary()),
            ),
        ])),
        Rect { height: 1, ..inner },
    );

    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(colour))
        .ratio(soc.map(|s| s / 100.0).unwrap_or(0.0))
        .label(
            soc.map(|s| format!("{s:.0}%"))
                .unwrap_or_else(|| "--".to_string()),
        );
    frame.render_widget(
        gauge,
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );

    let fahrenheit =
        app.config.ui.temperature_unit == crate::config::settings::TempUnit::Fahrenheit;
    // The station's state gives the runtime its meaning, and the label says where the
    // figure came from: a device counter, a calculation here, or nothing.
    let est = estimate(
        b,
        &snap.totals,
        &snap.samples,
        // The device reports its own charge limit; a station that charges to 85% is
        // "full" at 85%, not at 100%.
        d.settings.max_charge_soc.unwrap_or(100.0),
        snap.at,
    );
    let runtime_mark = match est.provenance {
        Provenance::DeviceEstimate => "device-reported",
        Provenance::Computed => "calculated here",
        _ => "no estimate",
    };
    let runtime_short = match (est.state, est.minutes) {
        (State::Charging, Some(m)) => format!("full in {}", duration_min(Some(m))),
        (State::Discharging, Some(m)) => format!("runs {}", duration_min(Some(m))),
        (s, _) => s.label().to_string(),
    };
    // A counter pinned at 99 h 59 m with nothing to calculate from is a missing
    // estimate, not a runtime, so the row flags it instead of showing a number.
    let pinned = est.raw.map(is_sentinel).unwrap_or(false) && est.minutes.is_none();
    let runtime_colour = if pinned {
        theme.warning()
    } else {
        theme.accent()
    };

    let mut rows: Vec<Line> = Vec::new();
    rows.push(Line::from(vec![
        Span::styled(
            if tight { "" } else { "remaining " },
            Style::default().fg(theme.secondary()),
        ),
        Span::styled(wh(b.remaining_wh), Style::default().fg(theme.text())),
        Span::styled(
            if tight { " / " } else { "  capacity " },
            Style::default().fg(theme.secondary()),
        ),
        Span::styled(wh(b.capacity_wh), Style::default().fg(theme.text())),
    ]));
    rows.push(Line::from(vec![
        Span::styled(
            if tight { "soh " } else { "health " },
            Style::default().fg(theme.secondary()),
        ),
        Span::styled(percent(b.soh), Style::default().fg(theme.text())),
        Span::styled("  cycles ", Style::default().fg(theme.secondary())),
        Span::styled(
            b.cycles
                .map(|c| grouped(c as i64))
                .unwrap_or_else(|| "--".to_string()),
            Style::default().fg(theme.text()),
        ),
        Span::styled(
            if tight { "" } else { "  temp " },
            Style::default().fg(theme.secondary()),
        ),
        Span::styled(
            if tight {
                String::new()
            } else {
                temp(b.temp_c, fahrenheit)
            },
            Style::default().fg(theme.text()),
        ),
    ]));
    rows.push(Line::from(vec![
        Span::styled(
            if tight { "time " } else { "runtime " },
            Style::default().fg(theme.secondary()),
        ),
        Span::styled(
            if tight {
                runtime_short.clone()
            } else {
                est.label()
            },
            Style::default()
                .fg(runtime_colour)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if tight {
                String::new()
            } else {
                format!("  {runtime_mark}")
            },
            Style::default().fg(theme.secondary()),
        ),
    ]));

    let state = match est.state {
        State::Charging => ("CHARGING", theme.charging()),
        State::Discharging => ("DISCHARGING", theme.accent()),
        State::Idle => ("IDLE", theme.secondary()),
        State::Full => ("FULL", theme.charging()),
        State::Unknown => ("state unknown", theme.secondary()),
    };
    rows.push(Line::from(vec![
        Span::styled(
            state.0,
            Style::default().fg(state.1).add_modifier(Modifier::BOLD),
        ),
        // The power totals belong to the POWER FLOW panel; this row says what the
        // station is doing.
    ]));

    let kits: Vec<_> = d.active_kits().collect();
    if !kits.is_empty() {
        let mut line = vec![Span::styled(
            "extra packs ",
            Style::default().fg(theme.secondary()),
        )];
        for k in kits.iter().take(4) {
            line.push(Span::styled(
                format!(
                    "{} {}% ",
                    crate::utils::formatting::mask_serial(&k.sn),
                    k.soc as i64
                ),
                Style::default().fg(theme.text()),
            ));
        }
        rows.push(Line::from(line));
    }

    rows.push(Line::from(vec![
        Span::styled(
            format!("{} · ", d.source.label()),
            Style::default().fg(if d.source.is_mock() {
                theme.mock()
            } else {
                theme.secondary()
            }),
        ),
        Span::styled(
            format!(
                "updated {}",
                crate::utils::formatting::elapsed(d.seconds_since_update(snap.at))
            ),
            Style::default().fg(theme.secondary()),
        ),
    ]));

    let room = inner.height.saturating_sub(2) as usize;
    rows.truncate(room);
    frame.render_widget(
        Paragraph::new(rows),
        Rect {
            y: inner.y + 2,
            height: inner.height - 2,
            ..inner
        },
    );
}

pub fn raw_rows(d: &DeviceSnapshot, filter: &str, limit: usize) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = d
        .raw
        .iter()
        .filter(|(k, _)| filter.is_empty() || k.contains(filter))
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Array(a) => format!("[{}]", a.len()),
                serde_json::Value::Object(o) => format!("{{{}}}", o.len()),
                serde_json::Value::Null => "null".to_string(),
            };
            (k.clone(), text)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.truncate(limit);
    rows
}
