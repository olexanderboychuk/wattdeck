use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::Screen;

/// The computed panel areas for one render.
#[derive(Debug, Clone, Copy, Default)]
pub struct Areas {
    pub header: Rect,
    pub strip: Rect,
    pub main: Rect,
    pub history: Rect,
    pub footer: Rect,
    pub compact: bool,
}

/// Responsive split. Narrow terminals drop the side-by-side panels and shrink the
/// device strip to one line; short terminals drop the history band.
pub fn compute(area: Rect, screen: &Screen, device_count: usize) -> Areas {
    let compact = area.width < 72;
    let tight = area.height < 18;

    let header_h = if tight { 1 } else { 2 };
    // Only the screens that draw something in the strip band get one: device cards
    // on the dashboard and device list, the range selector on analytics.
    let strip_h = match screen {
        Screen::Analytics => 1,
        Screen::Dashboard | Screen::Devices => {
            if device_count <= 1 {
                0
            } else if compact {
                2
            } else {
                device_count.min(3) as u16 + 1
            }
        }
        _ => 0,
    };
    let history_h = if tight || compact { 0 } else { 4 };

    let base = [
        Constraint::Length(header_h),
        Constraint::Length(strip_h),
        Constraint::Min(6),
        Constraint::Length(history_h),
        Constraint::Length(1),
    ];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(base)
        .split(area);
    let main = chunks[2];

    Areas {
        header: chunks[0],
        strip: chunks[1],
        main,
        history: chunks[3],
        footer: chunks[4],
        compact,
    }
}

/// Split the main area into left/right columns. Narrow terminals give the
/// right column zero width, so the screens render a single panel.
pub fn columns(area: Rect, compact: bool) -> (Rect, Rect) {
    let constr = if compact {
        [Constraint::Min(6), Constraint::Length(0)]
    } else {
        [Constraint::Fill(1), Constraint::Fill(1)]
    };
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constr)
        .split(area);
    (chunks[0], chunks[1])
}

pub fn rows(area: Rect, top: Constraint, bottom: Constraint) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([top, bottom])
        .split(area);
    (chunks[0], chunks[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn wide_terminal_gets_all_bands() {
        let a = compute(rect(100, 30), &Screen::Dashboard, 2);
        assert!(a.header.height >= 2);
        assert!(a.strip.height >= 3);
        assert!(a.main.height >= 6);
        assert!(a.history.height == 4);
        assert!(a.footer.height == 1);
        assert!(!a.compact);
    }

    #[test]
    fn narrow_terminal_compacts() {
        let a = compute(rect(60, 30), &Screen::Dashboard, 2);
        assert!(a.compact);
        assert_eq!(a.history.height, 0);
        assert_eq!(a.strip.height, 2);
    }

    #[test]
    fn short_terminal_drops_history() {
        let a = compute(rect(100, 12), &Screen::Dashboard, 2);
        assert_eq!(a.history.height, 0);
        assert_eq!(a.header.height, 1);
    }

    #[test]
    fn single_device_has_no_strip() {
        let a = compute(rect(100, 30), &Screen::Dashboard, 1);
        assert_eq!(a.strip.height, 0);
    }

    #[test]
    fn areas_never_exceed_the_frame() {
        for size in [(20, 5), (40, 10), (80, 20), (120, 40)] {
            let area = rect(size.0, size.1);
            let a = compute(area, &Screen::Dashboard, 3);
            for r in [a.header, a.strip, a.main, a.history, a.footer] {
                assert!(r.right() <= area.right(), "{r:?} in {area:?}");
                assert!(r.bottom() <= area.bottom(), "{r:?} in {area:?}");
            }
        }
    }

    #[test]
    fn columns_split_evenly_when_wide() {
        let (l, r) = columns(rect(100, 10), false);
        assert_eq!(l.width + r.width, 100);
        assert!(l.width >= 40);
        let (l2, r2) = columns(rect(50, 10), true);
        assert_eq!(r2.width, 0);
        assert_eq!(l2.width, 50);
    }
}
