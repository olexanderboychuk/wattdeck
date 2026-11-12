use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::snapshot::Snapshot;
use crate::ui::widgets;

/// The dashboard: device list, the two primary panels, the history band, and the
/// diagnostics and connection pair. The layout band sizes come from `layout::compute`,
/// and every panel draws from the same `Snapshot`.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, areas: Areas) {
    widgets::device_cards::draw(app, snap, frame, areas.strip);
    widgets::battery::draw(app, snap, frame, areas.overview);
    widgets::power_flow::draw(app, snap, frame, areas.flow);
    widgets::power_graph::history(app, snap, frame, areas.history);
    widgets::diagnostics::draw(app, snap, frame, areas.diagnostics);
    widgets::connection::draw(app, snap, frame, areas.connection);
}
