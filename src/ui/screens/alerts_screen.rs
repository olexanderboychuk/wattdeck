use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::widgets::alert_list;

pub fn draw(app: &App, frame: &mut Frame, areas: Areas) {
    alert_list::draw(app, frame, areas.main);
}
