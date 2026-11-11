use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::utils::formatting::{pad, watts};

/// Power flow: inputs on the left, battery in the middle, outputs on the right.
/// Solar is blue, AC is cyan, DC yellow when it is drawing, totals are bold.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    if area.height < 3 || area.width < 6 {
        return;
    }
    let Some(d) = app.selected_device() else {
        return;
    };
    let block = Block::default()
        .title(" POWER FLOW ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let p = &d.power;
    let mut lines: Vec<Line> = Vec::new();

    let solar = p.solar_in_w.unwrap_or(0.0);
    let ac_in = p.ac_in_w.unwrap_or(0.0);
    let dc_in = p.dc_in_w.unwrap_or(0.0);

    lines.push(Line::from(vec![
        Span::styled(pad("Solar", 8), Style::default().fg(theme.solar())),
        Span::styled(watts(p.solar_in_w), Style::default().fg(theme.solar())),
        Span::styled(
            format!("   AC {}", watts(p.ac_in_w)),
            Style::default().fg(theme.accent()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            pad("TOTAL IN", 8),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            watts(p.total_in_w),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   DC {}", watts(p.dc_in_w)),
            Style::default().fg(theme.secondary()),
        ),
    ]));

    let in_w = p.total_in_w.unwrap_or(solar + ac_in + dc_in);
    let out_w = p.total_out_w.unwrap_or(0.0);
    // Direction comes from the balance the device reports, not from "any input is
    // non-zero": a panel can be charging from solar while it also feeds a load.
    let net = in_w - out_w;
    let flow = if net > 5.0 {
        "──▶ charging"
    } else if net < -5.0 {
        "◀── discharging"
    } else {
        "──  idle"
    };
    lines.push(Line::from(Span::styled(
        format!("{} {}%", flow, d.battery.soc.unwrap_or(0.0) as i64),
        Style::default().fg(theme.accent()),
    )));

    lines.push(Line::from(vec![
        Span::styled(pad("AC out", 8), Style::default().fg(theme.accent())),
        Span::styled(watts(p.ac_out_w), Style::default().fg(theme.accent())),
        Span::styled(
            format!("   USB {}", watts(p.usb_out_w)),
            Style::default().fg(theme.secondary()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            pad("TOTAL OUT", 8),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            watts(p.total_out_w),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   12V {}", watts(p.car_out_w)),
            Style::default().fg(theme.secondary()),
        ),
    ]));

    // Port detail, only when the device reports individual ports.
    if !p.ports.is_empty() {
        let mut row: Vec<Span> = Vec::new();
        for (label, w) in p.ports.iter().take(6) {
            row.push(Span::styled(
                format!("{label} {}  ", watts(Some(*w))),
                Style::default().fg(theme.secondary()),
            ));
        }
        lines.push(Line::from(row));
    }

    // Electrical detail when reported.
    if p.ac_in_volts.is_some() || p.ac_out_volts.is_some() {
        lines.push(Line::from(vec![
            Span::styled("AC in ", Style::default().fg(theme.secondary())),
            Span::styled(
                format!("{} {}", volts_opt(p.ac_in_volts), amps_opt(p.ac_in_amps)),
                Style::default().fg(theme.secondary()),
            ),
            Span::styled("  AC out ", Style::default().fg(theme.secondary())),
            Span::styled(
                format!("{} {}", volts_opt(p.ac_out_volts), amps_opt(p.ac_out_amps)),
                Style::default().fg(theme.secondary()),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn volts_opt(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.1} V"))
        .unwrap_or_else(|| "--".to_string())
}

fn amps_opt(a: Option<f64>) -> String {
    a.map(|a| format!("{a:.2} A"))
        .unwrap_or_else(|| "--".to_string())
}
