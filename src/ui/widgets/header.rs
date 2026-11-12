use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::services::ConnState;
use crate::ui::widgets::pad_right;
use crate::utils::formatting::elapsed;

/// The top line: the app's own name and version, the mock badge, and every
/// connection state, each in its own semantic colour. Split out of `draw` so the
/// header text is testable without a terminal.
/// wattdeck is the app; the vendor name has no business being in the app's title.
pub fn title(app: &App) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let overall = app.connection.overall();
    let status_colour = match overall {
        ConnState::Connected => theme.online(),
        ConnState::Degraded => theme.warning(),
        ConnState::Offline | ConnState::AuthError => theme.critical(),
        _ => theme.secondary(),
    };
    let mut top: Vec<Span> = vec![
        Span::styled(
            "WATTDECK",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.secondary()),
        ),
    ];
    if app.mock {
        top.push(Span::styled(
            " MOCK",
            Style::default()
                .fg(theme.mock())
                .add_modifier(Modifier::BOLD),
        ));
    }
    // The two transports say the state in full; the summary word is only worth
    // space when something is not right.
    if overall != ConnState::Connected {
        top.push(Span::styled(
            format!(" {}", overall.label()),
            Style::default()
                .fg(status_colour)
                .add_modifier(Modifier::BOLD),
        ));
    }
    for (label, state) in [
        ("cloud", app.connection.rest()),
        ("live data", app.connection.mqtt()),
    ] {
        let colour = match state {
            ConnState::Connected => theme.charging(),
            ConnState::AuthError => theme.critical(),
            ConnState::Degraded | ConnState::Reconnecting => theme.warning(),
            ConnState::Offline => theme.offline(),
            _ => theme.secondary(),
        };
        top.push(Span::styled(
            format!(" {label} {}", state.label()),
            Style::default().fg(colour),
        ));
    }
    top
}

/// The second line: how many devices the account has, which one is on screen, and
/// how old its numbers are.
pub fn status_line(app: &App) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let fresh = app
        .freshness()
        .map(elapsed)
        .unwrap_or_else(|| "never".to_string());
    let selected = app
        .selected_device()
        .map(|d| d.product_name.clone())
        .unwrap_or_else(|| "no device".to_string());
    vec![
        Span::styled(
            format!(" DEVICES {} ", app.devices.len()),
            Style::default().fg(theme.text()),
        ),
        Span::styled(
            format!("SEL {selected} "),
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            format!("UPDATED {fresh} "),
            Style::default().fg(if app.stale() {
                theme.warning()
            } else {
                theme.secondary()
            }),
        ),
    ]
}

/// Active alerts, and the wall-clock time of the render, both pinned to the right
/// edge so they sit in the same columns every frame.
pub fn right_line(app: &App) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let criticals = app.unacknowledged_criticals();
    let alert_colour = if criticals > 0 {
        theme.critical()
    } else if app.alert_count() > 0 {
        theme.warning()
    } else {
        theme.secondary()
    };
    let mut right = Vec::new();

    if criticals > 0 {
        right.push(Span::styled(
            format!("ALERTS {} CRITICAL ", app.alert_count()),
            Style::default()
                .fg(alert_colour)
                .add_modifier(Modifier::BOLD),
        ));
    } else if app.alert_count() > 0 {
        right.push(Span::styled(
            format!("ALERTS {} ", app.alert_count()),
            Style::default().fg(alert_colour),
        ));
    }
    right.push(Span::styled(
        format!("{} UTC ", app.now.format("%H:%M:%S")),
        Style::default().fg(theme.secondary()),
    ));
    right
}

/// Two-line header: identity and connection state, then device/selection freshness
/// with the clock and the alert count at the right edge.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let top = pad_right(title(app), right_line(app), area.width);
    frame.render_widget(Paragraph::new(top), area);

    if area.height < 2 {
        return;
    }
    // The transport columns already state where the data comes from, so live mode
    // adds no source label; the MOCK badge is the mock-mode marker.
    let mut left = status_line(app);
    left.push(Span::styled(
        app.status.to_string(),
        Style::default().fg(app.theme.secondary()),
    ));
    // The alert count and clock live on the top line, so the bottom line shows the
    // status text alone at the right edge when the two fit.
    let bottom = Line::from(left);
    frame.render_widget(
        Paragraph::new(bottom),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;
    use crate::services::telemetry_service::Scenario;

    fn app(mock: bool) -> App {
        App::new(Config::default(), mock, Scenario::SolarCharging)
    }

    fn top_text(app: &App) -> String {
        title(app).iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn header_is_the_app_name() {
        let t = top_text(&app(true));
        assert!(t.starts_with("WATTDECK"), "{t}");
        assert!(t.contains("MOCK"), "mock mode is badged: {t}");
        assert!(
            !t.contains("ECOFLOW"),
            "the vendor is not in the app's title: {t}"
        );
    }

    #[test]
    fn live_header_carries_no_mock_badge() {
        let t = top_text(&app(false));
        assert!(t.starts_with("WATTDECK"), "{t}");
        assert!(!t.contains("MOCK"), "{t}");
    }

    #[test]
    fn header_carries_the_version_and_both_transports() {
        let t = top_text(&app(false));
        assert!(t.contains(env!("CARGO_PKG_VERSION")), "{t}");
        assert!(t.contains("cloud"), "{t}");
        assert!(t.contains("live data"), "{t}");
    }

    #[test]
    fn clock_and_alerts_are_right_aligned() {
        let line = pad_right(status_line(&app(true)), right_line(&app(true)), 60);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("UTC "), "clock sits at the edge: {text}");
        assert_eq!(
            text.chars().count(),
            60,
            "padded to the frame width: {text}"
        );
    }
}
