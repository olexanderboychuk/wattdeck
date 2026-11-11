use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::widgets;

pub fn draw(app: &App, frame: &mut Frame, areas: Areas) {
    widgets::device_cards::draw(app, frame, areas.strip);

    // Main area: battery on the left, power on the right; narrow terminals stack them.
    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    widgets::battery::draw(app, frame, left);
    if right.width > 0 {
        let (flow, dist) = crate::ui::layout::rows(
            right,
            ratatui::layout::Constraint::Fill(2),
            ratatui::layout::Constraint::Fill(1),
        );
        widgets::power_flow::draw(app, frame, flow);
        widgets::power_graph::distribution(app, frame, dist);
    }

    widgets::power_graph::history(app, frame, areas.history);
}
