pub mod alert_list;
pub mod battery;
pub mod connection;
pub mod device_cards;
pub mod diagnostics;
pub mod footer;
pub mod header;
pub mod metrics;
pub mod power_flow;
pub mod power_graph;

use ratatui::text::{Line, Span};

/// Right-align `right` inside `width`. The fixed columns mean the clock, the alert
/// count and the footer hints sit in the same place every frame, which is what
/// makes a monitoring dashboard readable at a glance.
pub fn pad_right(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let len = |spans: &[Span<'static>]| -> usize {
        spans.iter().map(|s| s.content.chars().count()).sum()
    };
    let mut spans = left;
    let used = len(&spans) + len(&right);
    if used < width as usize {
        spans.push(Span::from(" ".repeat(width as usize - used)));
    }
    spans.extend(right);
    Line::from(spans)
}
