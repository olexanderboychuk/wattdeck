use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Paragraph, Sparkline};

use crate::app::App;
use crate::ui::snapshot::Snapshot;
use crate::ui::theme::Theme;
use crate::ui::widgets::pad_right;
use crate::utils::formatting::{elapsed, watts};

/// The power history band: the live numbers, the peaks, and the chart itself. The
/// chart is drawn when there are four rows of plot area and 24 columns, sparklines
/// when there is less, and a plain statement when the sample window is too short
/// to plot.
pub fn history(app: &App, snap: &Snapshot, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if area.height < 4 || area.width < 12 {
        return;
    }
    if snap.device.is_none() {
        return;
    }
    let block = Block::default()
        .title(" POWER HISTORY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Row 0 is always the live reading, so the chart's right edge is labelled.
    frame.render_widget(
        Paragraph::new(summary_line(snap, theme, inner.width)),
        Rect { height: 1, ..inner },
    );

    let plot = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };

    if snap.samples.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "collecting samples",
                Style::default().fg(theme.secondary()),
            )),
            plot,
        );
        return;
    }
    // One sample is drawn as a single bar; a chart with one point on a time axis
    // says nothing, so the narrow form is used until there are two.
    if snap.samples.len() >= 2 && plot.height >= 4 && plot.width >= 26 {
        draw_chart(snap, theme, frame, plot);
    } else {
        draw_sparklines(snap, theme, frame, plot);
    }
}

/// Live values, peaks and the window the chart covers, with the live numbers
/// coloured to match the two series.
fn summary_line(snap: &Snapshot, theme: &Theme, width: u16) -> Line<'static> {
    let live = vec![
        Span::styled(
            format!(
                "live IN {} ({})  ",
                watts(snap.totals.in_w),
                Snapshot::mark(snap.totals.in_src)
            ),
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            format!(
                "OUT {} ({})",
                watts(snap.totals.out_w),
                Snapshot::mark(snap.totals.out_src)
            ),
            Style::default().fg(theme.warning()),
        ),
    ];
    let n = snap.samples.len();
    let right = vec![Span::styled(
        format!(
            " peak IN {} OUT {} · {} over {} · edge {}",
            watts(snap.peak_in),
            watts(snap.peak_out),
            if n == 1 {
                "1 sample".to_string()
            } else {
                format!("{n} samples")
            },
            elapsed(snap.window_secs()),
            snap.latest
                .map(|s| s.t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "--".to_string()),
        ),
        Style::default().fg(theme.secondary()),
    )];
    pad_right(live, right, width)
}

fn draw_chart(snap: &Snapshot, theme: &Theme, frame: &mut Frame, area: Rect) {
    let t0 = snap.samples[0].t;
    let span = (snap.samples[snap.samples.len() - 1].t - t0)
        .num_seconds()
        .max(1) as f64;
    let peak = snap
        .peak_in
        .unwrap_or(0.0)
        .max(snap.peak_out.unwrap_or(0.0))
        .max(1.0);
    // Round the y bound up to the next 100 W so the top line is not clipped.
    let ymax = (peak / 100.0).ceil() * 100.0;

    let in_pts: Vec<(f64, f64)> = snap
        .samples
        .iter()
        .map(|s| ((s.t - t0).num_seconds() as f64, s.in_w.max(0.0)))
        .collect();
    let out_pts: Vec<(f64, f64)> = snap
        .samples
        .iter()
        .map(|s| ((s.t - t0).num_seconds() as f64, s.out_w.max(0.0)))
        .collect();

    let mid = t0 + chrono::Duration::seconds((span / 2.0) as i64);
    let axis_style = Style::default().fg(theme.secondary());
    let chart = Chart::new(vec![
        Dataset::default()
            .name("IN")
            .marker(symbols::Marker::Dot)
            .data(&in_pts)
            .style(Style::default().fg(theme.accent())),
        Dataset::default()
            .name("OUT")
            .marker(symbols::Marker::Dot)
            .data(&out_pts)
            .style(Style::default().fg(theme.warning())),
    ])
    .x_axis(
        Axis::default()
            .bounds([0.0, span])
            .labels(vec![
                t0.format("%H:%M:%S").to_string(),
                mid.format("%H:%M:%S").to_string(),
                snap.samples[snap.samples.len() - 1]
                    .t
                    .format("%H:%M:%S")
                    .to_string(),
            ])
            .style(axis_style),
    )
    .y_axis(
        Axis::default()
            .bounds([0.0, ymax])
            .labels(vec![
                "0".to_string(),
                watts(Some(ymax / 2.0)),
                watts(Some(ymax)),
            ])
            .style(axis_style),
    );
    frame.render_widget(chart, area);
}

/// The narrow fallback: two bar rows, one per series, labelled with mean and peak
/// so the bars are not read as energy.
fn draw_sparklines(snap: &Snapshot, theme: &Theme, frame: &mut Frame, area: Rect) {
    let in_data: Vec<u64> = snap
        .samples
        .iter()
        .map(|s| s.in_w.max(0.0).clamp(0.0, u16::MAX as f64) as u64)
        .collect();
    let out_data: Vec<u64> = snap
        .samples
        .iter()
        .map(|s| s.out_w.max(0.0).clamp(0.0, u16::MAX as f64) as u64)
        .collect();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Min(1)])
        .split(area);

    let bar_set = if theme.unicode {
        symbols::bar::NINE_LEVELS
    } else {
        symbols::bar::THREE_LEVELS
    };
    frame.render_widget(
        Sparkline::default()
            .data(&in_data)
            .bar_set(bar_set.clone())
            .style(Style::default().fg(theme.accent())),
        rows[0],
    );
    frame.render_widget(
        Sparkline::default()
            .data(&out_data)
            .bar_set(bar_set.clone())
            .style(Style::default().fg(theme.warning())),
        rows[1],
    );

    let label_w = (area.width as usize).min(40);
    let label = Line::from(vec![
        Span::styled(
            format!(
                "IN avg {} peak {} ",
                watts(snap.mean_in),
                watts(snap.peak_in)
            ),
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            format!(
                "OUT avg {} peak {}",
                watts(snap.mean_out),
                watts(snap.peak_out)
            ),
            Style::default().fg(theme.warning()),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(label).style(Style::default().bg(theme.bg())),
        Rect {
            x: area.x + area.width - label_w as u16,
            y: area.y,
            width: label_w as u16,
            height: 1,
        },
    );
}
