use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::snapshot::Snapshot;
use crate::ui::widgets;

pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, areas: Areas) {
    // The device list, expanded, plus the overview and flow panels of the selected
    // device, so the two views stay consistent.
    widgets::device_cards::draw(app, snap, frame, areas.strip);
    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);
    widgets::battery::draw(app, snap, frame, left);
    if right.width > 0 {
        widgets::power_flow::draw(app, snap, frame, right);
    }
    widgets::power_graph::history(app, snap, frame, areas.history);
}
