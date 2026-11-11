use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::telemetry::models::DeviceSnapshot;
use crate::utils::formatting::{amps, duration_min, grouped, percent, temp, volts, watts, wh};

/// Battery panel: the headline gauge, energy figures, and the diagnostics the
/// device actually reports. Fields the device never sent are shown as `--`, never
/// as a fabricated zero.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // A bordered panel needs at least one row of content; drawing in a smaller area
    // writes outside the buffer.
    if area.height < 3 || area.width < 6 {
        return;
    }
    let Some(d) = app.selected_device() else {
        frame.render_widget(
            Paragraph::new("no device selected").style(Style::default().fg(theme.secondary())),
            area,
        );
        return;
    };
    let title = format!(" {} ", d.product_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let b = &d.battery;
    let soc = b.soc;
    let bar_width = (inner.width.saturating_sub(2)).clamp(10, 40) as usize;
    let bar_colour = soc
        .map(|s| {
            theme.battery_colour(
                s,
                app.config.alerts.battery_low_percent,
                app.config.alerts.battery_critical_percent,
            )
        })
        .unwrap_or(theme.secondary());
    let gauge = theme.bar(bar_width, soc.unwrap_or(0.0) / 100.0);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(gauge, Style::default().fg(bar_colour)),
        Span::styled(
            format!(" {}", percent(soc)),
            Style::default().fg(bar_colour).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("remaining ", Style::default().fg(theme.secondary())),
        Span::styled(wh(b.remaining_wh), Style::default().fg(theme.text())),
        Span::styled("  capacity ", Style::default().fg(theme.secondary())),
        Span::styled(wh(b.capacity_wh), Style::default().fg(theme.text())),
    ]));

    let runtime = if let Some(min) = b.remain_time_min {
        format!("{} (device)", duration_min(Some(min)))
    } else if let Some(min) = b.computed_runtime_min() {
        format!("{} (calc)", duration_min(Some(min)))
    } else {
        "--".to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("health ", Style::default().fg(theme.secondary())),
        Span::styled(percent(b.soh), Style::default().fg(theme.text())),
        Span::styled("  temp ", Style::default().fg(theme.secondary())),
        Span::styled(
            temp(
                b.temp_c,
                app.config.ui.temperature_unit == crate::config::settings::TempUnit::Fahrenheit,
            ),
            Style::default().fg(theme.text()),
        ),
        Span::styled("  runtime ", Style::default().fg(theme.secondary())),
        Span::styled(runtime, Style::default().fg(theme.accent())),
    ]));

    if app.config.history.memory_samples > 0 {
        // Cell block: min/max spread is what matters for a glanceable dashboard.
        let spread = match (b.min_cell_volts(), b.max_cell_volts()) {
            (Some(min), Some(max)) => format!("{:.3}–{:.3} V", min, max),
            _ => "--".to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled("cells ", Style::default().fg(theme.secondary())),
            Span::styled(spread, Style::default().fg(theme.text())),
            Span::styled("  cycles ", Style::default().fg(theme.secondary())),
            Span::styled(
                b.cycles
                    .map(|c| grouped(c as i64))
                    .unwrap_or_else(|| "--".to_string()),
                Style::default().fg(theme.text()),
            ),
            Span::styled("  I ", Style::default().fg(theme.secondary())),
            Span::styled(volts(b.volt_v), Style::default().fg(theme.text())),
            Span::styled(" ", Style::default().fg(theme.secondary())),
            Span::styled(amps(b.amp_a), Style::default().fg(theme.text())),
        ]));
    }

    // Charge state line, colour-coded.
    let state = match (b.charging, b.discharging) {
        (Some(true), _) => ("CHARGING", theme.charging()),
        (Some(false), Some(true)) => ("DISCHARGING", theme.accent()),
        (Some(false), Some(false)) => ("IDLE", theme.secondary()),
        _ => ("unknown", theme.secondary()),
    };
    lines.push(Line::from(Span::styled(
        format!(
            "{} · IN {} · OUT {}",
            state.0,
            watts(d.power.total_in_w),
            watts(d.power.total_out_w)
        ),
        Style::default().fg(state.1),
    )));

    // Extra battery packs.
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
        lines.push(Line::from(line));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Cell voltage helpers for the diagnostics line, reading the merged raw map.
trait CellExt {
    fn min_cell_volts(&self) -> Option<f64>;
    fn max_cell_volts(&self) -> Option<f64>;
}

impl CellExt for crate::telemetry::models::Battery {
    fn min_cell_volts(&self) -> Option<f64> {
        self.cell_volts
            .iter()
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .copied()
    }
    fn max_cell_volts(&self) -> Option<f64> {
        self.cell_volts
            .iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .copied()
    }
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
