use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::services::ConnState;
use crate::utils::formatting::elapsed;

/// The top line: the app's own name, the mock badge, and the connection states.
/// Split out of `draw` so the header text is testable without a terminal.
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
    let mut top: Vec<Span> = vec![Span::styled(
        "WATTDECK",
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD),
    )];
    if app.mock {
        top.push(Span::styled(
            " MOCK",
            Style::default()
                .fg(theme.mock())
                .add_modifier(Modifier::BOLD),
        ));
    }
    top.push(Span::styled(
        format!(" ● {} ", overall.label()),
        Style::default()
            .fg(status_colour)
            .add_modifier(Modifier::BOLD),
    ));
    for (label, state) in [
        ("REST", app.connection.rest()),
        ("MQTT", app.connection.mqtt()),
    ] {
        let colour = match state {
            ConnState::Connected => theme.charging(),
            ConnState::AuthError => theme.critical(),
            ConnState::Degraded | ConnState::Reconnecting => theme.warning(),
            ConnState::Offline => theme.offline(),
            _ => theme.secondary(),
        };
        top.push(Span::styled(
            format!("{label} {} ", state.label()),
            Style::default().fg(colour),
        ));
    }
    top
}

/// Two-line header: identity and connection state, then device/selection freshness.
pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let top = title(app);
    frame.render_widget(Paragraph::new(Line::from(top)), area);

    if area.height < 2 {
        return;
    }
    // The transport columns already state where the data comes from, so live mode
    // adds no source label; the MOCK badge is the mock-mode marker.
    let fresh = app
        .freshness()
        .map(elapsed)
        .unwrap_or_else(|| "never".to_string());
    let selected = app
        .selected_device()
        .map(|d| d.product_name.clone())
        .unwrap_or_else(|| "no device".to_string());
    let criticals = app.unacknowledged_criticals();
    let alert_colour = if criticals > 0 {
        theme.critical()
    } else if app.alert_count() > 0 {
        theme.warning()
    } else {
        theme.secondary()
    };
    let bottom = Line::from(vec![
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
        Span::styled(
            format!("ALERTS {} ", app.alert_count()),
            Style::default().fg(alert_colour),
        ),
        Span::styled(
            app.status.to_string(),
            Style::default().fg(theme.secondary()),
        ),
    ]);
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
}
