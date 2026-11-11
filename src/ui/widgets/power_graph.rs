use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline};

use crate::app::App;
use crate::telemetry::history::{mean_power, peak_powers};
use crate::utils::formatting::{pad, watts};

/// Share-of-total bars for the inputs and outputs, scaled to the largest channel.
pub fn distribution(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // Bars draws a block, a bar row and a label row; a shorter area overflows.
    if area.height < 5 || area.width < 8 {
        return;
    }
    let Some(d) = app.selected_device() else {
        return;
    };
    let block = Block::default()
        .title(" DISTRIBUTION ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let p = &d.power;
    let rows = [
        ("Solar", p.solar_in_w, theme.solar()),
        ("AC in", p.ac_in_w, theme.accent()),
        ("DC in", p.dc_in_w, theme.secondary()),
        ("AC out", p.ac_out_w, theme.accent()),
        ("USB out", p.usb_out_w, theme.secondary()),
        ("12V out", p.car_out_w, theme.secondary()),
    ];
    let scale = rows
        .iter()
        .filter_map(|(_, v, _)| *v)
        .fold(0.0f64, f64::max)
        .max(1.0);
    let bar_w = (inner.width.saturating_sub(18)).clamp(6, 28) as usize;

    let mut lines = Vec::new();
    for (label, value, colour) in rows {
        let v = value.unwrap_or(0.0);
        if value.is_none() {
            continue;
        }
        lines.push(Line::from(vec![
            Span::styled(pad(label, 7), Style::default().fg(colour)),
            Span::styled(theme.bar(bar_w, v / scale), Style::default().fg(colour)),
            Span::styled(
                format!(" {}", watts(value)),
                Style::default().fg(theme.text()),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no power channels reported",
            Style::default().fg(theme.secondary()),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Sparkline band for the recent power history. Values are instantaneous watts; the
/// label shows peak and mean so the chart is not read as energy.
pub fn history(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // Two sparkline rows plus the block border and the label row.
    if area.height < 4 || area.width < 8 {
        return;
    }
    let Some(d) = app.selected_device() else {
        return;
    };
    let samples = app.history_for(&d.sn).ordered();
    let block = Block::default()
        .title(" POWER HISTORY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if samples.len() < 2 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "insufficient history: not enough samples to chart (collecting)",
                Style::default().fg(theme.warning()),
            )),
            inner,
        );
        return;
    }

    // Values are watts clamped to the u64 range the Sparkline widget accepts.
    let in_data: Vec<u64> = samples
        .iter()
        .map(|s| s.in_w.max(0.0).clamp(0.0, u16::MAX as f64) as u64)
        .collect();
    let out_data: Vec<u64> = samples
        .iter()
        .map(|s| s.out_w.max(0.0).clamp(0.0, u16::MAX as f64) as u64)
        .collect();

    let (peak_in, peak_out) = peak_powers(&samples);
    let mean_in = mean_power(&samples, |s| s.in_w);
    let mean_out = mean_power(&samples, |s| s.out_w);

    let rows = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(inner);

    let spark_in = Sparkline::default()
        .data(&in_data)
        .bar_set(if theme.unicode {
            symbols::bar::NINE_LEVELS
        } else {
            symbols::bar::THREE_LEVELS
        })
        .style(Style::default().fg(theme.accent()));
    frame.render_widget(spark_in, rows[0]);

    let spark_out = Sparkline::default()
        .data(&out_data)
        .bar_set(if theme.unicode {
            symbols::bar::NINE_LEVELS
        } else {
            symbols::bar::THREE_LEVELS
        })
        .style(Style::default().fg(theme.warning()));
    frame.render_widget(spark_out, rows[1]);

    // Labels sit at the right end of each sparkline row so the bars stay readable
    // from the left.
    let label_w = (inner.width as usize).min(34);
    let label_area = ratatui::layout::Rect {
        x: inner.x + inner.width - label_w as u16,
        y: inner.y,
        width: label_w as u16,
        height: 1,
    };
    let label_in = Line::from(Span::styled(
        format!(
            "IN {} · peak {}",
            watts(Some(mean_in)),
            watts(Some(peak_in))
        ),
        Style::default().fg(theme.secondary()),
    ));
    frame.render_widget(
        Paragraph::new(label_in).style(Style::default().bg(theme.bg())),
        label_area,
    );
    let label_out = Line::from(Span::styled(
        format!(
            "OUT {} · peak {}",
            watts(Some(mean_out)),
            watts(Some(peak_out))
        ),
        Style::default().fg(theme.secondary()),
    ));
    let label_area2 = ratatui::layout::Rect {
        y: inner.y + 1,
        ..label_area
    };
    frame.render_widget(
        Paragraph::new(label_out).style(Style::default().bg(theme.bg())),
        label_area2,
    );
}
