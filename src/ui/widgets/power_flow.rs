use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::telemetry::models::Channel;
use crate::ui::snapshot::Snapshot;
use crate::ui::theme::Theme;
use crate::utils::formatting::{pad, watts};

/// Power flow: every channel the device reports, as a share bar and a wattage,
/// inputs on the left and outputs on the right. The totals line is the same
/// normalized total the overview and the history chart use.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if area.height < 3 || area.width < 6 {
        return;
    }
    let Some(d) = snap.device else {
        return;
    };
    let block = Block::default()
        .title(" POWER FLOW ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let scale = snap
        .channels
        .iter()
        .filter_map(|c| c.value)
        .fold(0.0f64, f64::max)
        .max(1.0);

    let mut rows: Vec<Line> = Vec::new();
    rows.push(totals_line(snap, theme, inner.width));

    let ins: Vec<&Channel> = snap.channels.iter().filter(|c| c.input).collect();
    let outs: Vec<&Channel> = snap.channels.iter().filter(|c| !c.input).collect();
    let pairs = ins.len().max(outs.len());
    // Two columns when there is room for a bar in each, otherwise one row per
    // channel. The bar length is what the eye compares, so it never shrinks below
    // six cells: below that the panel draws single rows.
    let half = inner.width / 2;
    let bar_w = half.saturating_sub(9 + 8) as usize;
    if inner.width >= 58 && bar_w >= 6 {
        for i in 0..pairs {
            let left = ins.get(i).map(|c| channel_line(c, bar_w, scale, theme));
            let right = outs.get(i).map(|c| channel_line(c, bar_w, scale, theme));
            let mut spans = Vec::new();
            spans.extend(fill_half(
                left.unwrap_or_else(|| blank_channel(half, theme)),
                half,
            ));
            spans.extend(fill_half(
                right.unwrap_or_else(|| blank_channel(half, theme)),
                half,
            ));
            rows.push(Line::from(spans));
        }
    } else {
        let bar_w = inner.width.saturating_sub(9 + 8) as usize;
        for c in snap.channels.iter() {
            if bar_w < 4 {
                rows.push(Line::from(single_channel_label(c, theme)));
            } else {
                rows.push(Line::from(channel_line(c, bar_w, scale, theme)));
            }
        }
    }

    // The net line is computed from the same totals the overview prints. The
    // device's own state word lives in the overview; a second verdict here would
    // duplicate it, and on an AC pass-through the computed net disagrees with the
    // device, so this line carries the number and the source marks only.
    if let (Some(i), Some(o)) = (snap.totals.in_w, snap.totals.out_w) {
        let net = i - o;
        let colour = if net > 5.0 {
            theme.charging()
        } else if net < -5.0 {
            theme.accent()
        } else {
            theme.secondary()
        };
        rows.push(Line::from(vec![
            Span::styled(
                format!("net {:>+5.0} W ", net),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " ({} → {})",
                    Snapshot::mark(snap.totals.in_src),
                    Snapshot::mark(snap.totals.out_src)
                ),
                Style::default().fg(theme.secondary()),
            ),
        ]));
    }

    // Port detail, only when the device reports individual ports.
    if !d.power.ports.is_empty() {
        let mut row: Vec<Span> = vec![Span::styled(
            "ports ",
            Style::default().fg(theme.secondary()),
        )];
        for (label, w) in d.power.ports.iter().take(6) {
            row.push(Span::styled(
                format!("{label} {} ", watts(Some(*w))),
                Style::default().fg(theme.secondary()),
            ));
        }
        rows.push(Line::from(row));
    }

    // Electrical detail when reported.
    if d.power.ac_in_volts.is_some() || d.power.ac_out_volts.is_some() {
        rows.push(Line::from(vec![Span::styled(
            format!(
                "AC in {} {}  AC out {} {}  {} Hz",
                volts_opt(d.power.ac_in_volts),
                amps_opt(d.power.ac_in_amps),
                volts_opt(d.power.ac_out_volts),
                amps_opt(d.power.ac_out_amps),
                d.power
                    .ac_out_freq_hz
                    .map(|f| format!("{f:.1}"))
                    .unwrap_or_else(|| "--".to_string()),
            ),
            Style::default().fg(theme.secondary()),
        )]));
    }
    if rows.len() == 1 {
        rows.push(Line::from(Span::styled(
            "no power channels reported",
            Style::default().fg(theme.secondary()),
        )));
    }

    let room = inner.height as usize;
    rows.truncate(room);
    frame.render_widget(Paragraph::new(rows), inner);
}

/// The two totals, bold, side by side.
fn totals_line(snap: &Snapshot, theme: &Theme, width: u16) -> Line<'static> {
    let left = format!(
        "{} ({})",
        watts(snap.totals.in_w),
        Snapshot::mark(snap.totals.in_src)
    );
    let right = format!(
        "{} ({})",
        watts(snap.totals.out_w),
        Snapshot::mark(snap.totals.out_src)
    );
    let bold = Style::default()
        .fg(theme.text())
        .add_modifier(Modifier::BOLD);
    let used = 9 + left.chars().count() + 10 + right.chars().count();
    let mut spans = vec![Span::styled(
        "TOTAL IN ",
        Style::default().fg(theme.secondary()),
    )];
    spans.push(Span::styled(left, bold));
    if width as usize > used {
        spans.push(Span::from(" ".repeat(width as usize - used)));
    }
    spans.push(Span::styled(
        "TOTAL OUT ",
        Style::default().fg(theme.secondary()),
    ));
    spans.push(Span::styled(right, bold));
    Line::from(spans)
}

fn colour_for(label: &str, theme: &Theme) -> ratatui::style::Color {
    match label {
        "solar in" => theme.solar(),
        "ac in" | "ac out" => theme.accent(),
        "dc in" | "dc out" => theme.warning(),
        _ => theme.secondary(),
    }
}

/// One channel: label, share bar, wattage. A channel the model does not have is
/// `n/a`; one the device has not reported this update is `--`; a measured zero
/// draws an empty bar with `0 W`, which is a different reading from both.
fn channel_line(c: &Channel, bar_w: usize, scale: f64, theme: &Theme) -> Vec<Span<'static>> {
    let colour = colour_for(c.label, theme);
    if !c.supported {
        return vec![
            Span::styled(pad(c.label, 9), Style::default().fg(theme.offline())),
            Span::styled("n/a".to_string(), Style::default().fg(theme.offline())),
            Span::from(" ".repeat(bar_w + 1)),
        ];
    }
    match c.value {
        None => vec![
            Span::styled(pad(c.label, 9), Style::default().fg(theme.secondary())),
            Span::styled("--".to_string(), Style::default().fg(theme.warning())),
            Span::from(" ".repeat(bar_w + 1)),
        ],
        Some(v) => vec![
            Span::styled(pad(c.label, 9), Style::default().fg(colour)),
            Span::styled(theme.bar(bar_w, v / scale), Style::default().fg(colour)),
            Span::styled(
                format!(" {}", watts(Some(v))),
                Style::default().fg(theme.text()),
            ),
        ],
    }
}

fn single_channel_label(c: &Channel, theme: &Theme) -> Span<'static> {
    let text = if !c.supported {
        format!("{} n/a", pad(c.label, 9))
    } else {
        format!("{} {}", pad(c.label, 9), watts(c.value))
    };
    Span::styled(text, Style::default().fg(theme.secondary()))
}

/// Pad a half-row to the column width, so the two columns line up.
fn fill_half(spans: Vec<Span<'static>>, half: u16) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let mut out = spans;
    if (used as u16) < half {
        out.push(Span::from(" ".repeat((half - used as u16) as usize)));
    }
    out
}

/// A half-row that keeps the columns lined up when one side has no channel.
fn blank_channel(half: u16, theme: &Theme) -> Vec<Span<'static>> {
    let _ = theme;
    vec![Span::from(" ".repeat(half as usize))]
}

fn volts_opt(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.1} V"))
        .unwrap_or_else(|| "--".to_string())
}

fn amps_opt(a: Option<f64>) -> String {
    a.map(|a| format!("{a:.2} A"))
        .unwrap_or_else(|| "--".to_string())
}
