pub mod layout;
pub mod screens;
pub mod theme;
pub mod widgets;

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{App, Screen};

/// One render pass. Called only when the UI is dirty, so idle terminals do no work.
pub fn render(app: &App, frame: &mut Frame) {
    let areas = layout::compute(frame.area(), &app.screen, app.devices.len());
    widgets::header::draw(app, frame, areas.header);
    match app.screen {
        Screen::Dashboard => screens::dashboard::draw(app, frame, areas),
        Screen::Devices => screens::devices::draw(app, frame, areas),
        Screen::DeviceDetails => screens::device_details::draw(app, frame, areas),
        Screen::Analytics => screens::analytics::draw(app, frame, areas),
        Screen::Alerts => screens::alerts_screen::draw(app, frame, areas),
        Screen::Settings => screens::settings::draw(app, frame, areas),
        Screen::Help => screens::help::draw(app, frame, areas),
    }
    widgets::footer::draw(app, frame, areas.footer);
    // The confirmation dialog sits over the screen, so the command the user is
    // about to send cannot be missed, and the screen behind stays readable.
    if let Some(text) = app.control_dialog() {
        let width = (text.chars().count() as u16 + 6).min(app_area_width(frame));
        let area = ratatui::layout::Rect {
            x: (frame.area().width - width) / 2,
            y: frame.area().height / 2,
            width,
            height: 3,
        };
        let block = ratatui::widgets::Block::default()
            .title(" CONFIRM ")
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(app.theme.warning()));
        let inner = ratatui::layout::Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width - 2,
            height: area.height - 2,
        };
        frame.render_widget(block, area);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(app.theme.text())
                    .add_modifier(Modifier::BOLD),
            ))),
            inner,
        );
    }
}

/// The panel width the dialog has to fit inside.
fn app_area_width(frame: &mut Frame) -> u16 {
    frame.area().width
}
