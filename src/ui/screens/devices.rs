use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::widgets;
use crate::ui::widgets::device_cards;

pub fn draw(app: &App, frame: &mut Frame, areas: Areas) {
    // The device list is the same strip, expanded, plus the detail panel of the
    // selected device, so the two views stay consistent.
    device_cards::draw(app, frame, areas.strip);
    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    crate::ui::widgets::battery::draw(app, frame, left);
    if right.width > 0 {
        crate::ui::widgets::power_flow::draw(app, frame, right);
    }
    widgets::power_graph::history(app, frame, areas.history);
}
