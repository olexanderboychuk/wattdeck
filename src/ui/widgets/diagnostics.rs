use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Paragraph};

use crate::app::App;
use crate::ui::snapshot::Snapshot;
use crate::ui::theme::Theme;
use crate::utils::formatting::{amps, grouped, percent, temp, volts, watts, wh};

/// BMS diagnostics: the electrical readings the device reports, and the cell
/// spread. A measurement the model does not carry is `n/a (model)`, one the device
/// has not sent is `--`, and a real zero is printed as a number.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if area.height < 3 || area.width < 12 {
        return;
    }
    let Some(d) = snap.device else {
        return;
    };
    let block = Block::default()
        .title(" BMS DIAGNOSTICS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let b = &d.battery;
    let fahrenheit =
        app.config.ui.temperature_unit == crate::config::settings::TempUnit::Fahrenheit;

    let mut rows: Vec<Line> = Vec::new();
    // Bus voltage and current, with the power the pair implies, marked as the app's
    // own arithmetic.
    let bus_w = match (b.volt_v, b.amp_a) {
        (Some(v), Some(a)) => format!("{} (calc)", watts(Some(v * a))),
        _ => "--".to_string(),
    };
    rows.push(kv(
        "battery",
        &format!("{} {} {}", volts(b.volt_v), amps(b.amp_a), bus_w),
        theme,
    ));

    let cells = match b.cell_range() {
        Some((min, max)) => format!(
            "{:.3}–{:.3} V  delta {:.0} mV  {} cells",
            min,
            max,
            (max - min) * 1000.0,
            b.valid_cells()
        ),
        _ if !snap.caps.battery_cells => "n/a (model)".to_string(),
        _ => "--".to_string(),
    };
    rows.push(kv("cells", &cells, theme));

    rows.push(Line::from(vec![
        Span::styled("cycles ", Style::default().fg(theme.secondary())),
        Span::styled(
            b.cycles
                .map(|c| grouped(c as i64))
                .unwrap_or_else(|| "--".to_string()),
            Style::default().fg(theme.text()),
        ),
        Span::styled("  soh ", Style::default().fg(theme.secondary())),
        Span::styled(percent(b.soh), Style::default().fg(theme.text())),
        Span::styled("  mos ", Style::default().fg(theme.secondary())),
        Span::styled(
            temp(b.max_mos_temp_c, fahrenheit),
            Style::default().fg(theme.text()),
        ),
    ]));

    let temps = match (b.min_cell_temp_c, b.max_cell_temp_c) {
        (Some(min), Some(max)) => format!(
            "pack {}  cells {}–{}",
            temp(b.temp_c, fahrenheit),
            temp(Some(min), fahrenheit),
            temp(Some(max), fahrenheit)
        ),
        _ => format!("pack {}", temp(b.temp_c, fahrenheit)),
    };
    rows.push(kv("temp", &temps, theme));

    rows.push(kv(
        "counters",
        &format!(
            "charged {}  discharged {}",
            wh(b.charged_kwh),
            wh(b.discharged_kwh)
        ),
        theme,
    ));

    let used = rows.len() as u16;
    let spare = inner.height.saturating_sub(used);
    // The cell spread is the one thing that reads spare rows as more telemetry: a
    // per-cell column chart, drawn from the values the BMS reported.
    let valid: Vec<(usize, f64)> = b
        .cell_volts
        .iter()
        .enumerate()
        .filter(|(_, v)| (2.0..=4.5).contains(*v))
        .map(|(i, v)| (i + 1, *v))
        .collect();
    if spare >= 3 && inner.width >= 20 && valid.len() > 1 && snap.caps.battery_cells {
        let (min, max) = b.cell_range().expect("more than one valid cell");
        let bars: Vec<Bar> = valid
            .iter()
            .map(|(slot, v)| {
                Bar::default()
                    .value(((v - min) * 1000.0).round() as u64)
                    .value_style(Style::default().fg(theme.accent()))
                    .label(Line::from(format!("{slot}")))
                    .text_value(format!("{:.2}", v))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(rows),
            Rect {
                height: used.min(inner.height),
                ..inner
            },
        );
        let chart = BarChart::default()
            .data(
                BarGroup::default()
                    .label(Line::from(format!("cell spread {:.3}–{:.3} V", min, max)))
                    .bars(&bars),
            )
            .bar_style(Style::default().fg(theme.accent()))
            .bar_set(if theme.unicode {
                symbols::bar::NINE_LEVELS
            } else {
                symbols::bar::THREE_LEVELS
            })
            .bar_gap(0)
            .bar_width(1)
            .max(((max - min) * 1000.0).max(1.0) as u64);
        frame.render_widget(
            chart,
            Rect {
                y: inner.y + used.min(inner.height),
                height: inner.height - used.min(inner.height),
                ..inner
            },
        );
        return;
    }

    let room = inner.height as usize;
    rows.truncate(room);
    frame.render_widget(Paragraph::new(rows), inner);
}

fn kv(key: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<8} "), Style::default().fg(theme.secondary())),
        Span::styled(
            value.to_string(),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
    ])
}
