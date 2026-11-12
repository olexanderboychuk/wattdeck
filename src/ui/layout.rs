use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::Screen;

/// The panel areas for one render. Every region is sized from the content it has
/// to hold, so a panel is never stretched to fill an area it cannot use.
#[derive(Debug, Clone, Copy, Default)]
pub struct Areas {
    pub header: Rect,
    pub strip: Rect,
    /// The band the non-dashboard screens lay their panels out in.
    pub main: Rect,
    pub overview: Rect,
    pub flow: Rect,
    pub history: Rect,
    pub diagnostics: Rect,
    pub connection: Rect,
    pub footer: Rect,
    /// Narrow terminals stack the panels in a single column.
    pub compact: bool,
}

/// Which band a slot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Primary,
    History,
    Diagnostics,
    Overview,
    Flow,
    Connection,
}

/// One vertical band: the rows it needs, the rows it can use, and how much of the
/// leftover it takes. Weight is what makes the layout content-aware: the chart
/// scales with spare rows, a text panel does not.
#[derive(Debug, Clone, Copy)]
struct Band {
    key: Key,
    min: u16,
    max: u16,
    weight: u16,
}

/// Responsive split. Wide terminals pair the panels side by side; narrow ones
/// stack them. Short terminals drop the bands that cannot show their content, in the
/// order history, diagnostics, connection, flow.
pub fn compute(area: Rect, screen: &Screen, device_count: usize) -> Areas {
    let compact = area.width < 88;
    let header_h = if area.height < 14 { 1 } else { 2 };
    let footer_h = 1;
    let strip_h = strip_height(screen, device_count, area.height);
    let avail = area.height.saturating_sub(header_h + strip_h + footer_h);

    // Display order. In the stacked layout the panels are their own bands.
    let mut bands = if compact {
        vec![
            Band {
                key: Key::Overview,
                min: 8,
                max: 12,
                weight: 3,
            },
            Band {
                key: Key::Flow,
                min: 8,
                max: 14,
                weight: 4,
            },
            Band {
                key: Key::Connection,
                min: 6,
                max: 10,
                weight: 2,
            },
            Band {
                key: Key::Diagnostics,
                min: 7,
                max: 12,
                weight: 1,
            },
            Band {
                key: Key::History,
                min: 6,
                max: 24,
                weight: 6,
            },
        ]
    } else {
        vec![
            Band {
                key: Key::Primary,
                min: 8,
                max: 10,
                weight: 2,
            },
            Band {
                key: Key::History,
                min: 6,
                max: 28,
                weight: 6,
            },
            Band {
                key: Key::Diagnostics,
                min: 8,
                max: 11,
                weight: 2,
            },
        ]
    };

    // Drop the least useful band until the minimums fit, then give the anchor band
    // whatever rows are left.
    let drop: Vec<Key> = if compact {
        vec![Key::History, Key::Diagnostics, Key::Connection, Key::Flow]
    } else {
        vec![Key::History, Key::Diagnostics]
    };
    for key in drop {
        if min_sum(&bands) <= avail {
            break;
        }
        bands.retain(|b| b.key != key);
    }
    if min_sum(&bands) > avail
        && let Some(first) = bands.get_mut(0)
    {
        first.min = avail;
        first.max = avail;
    }

    let lens = split(avail, &bands);
    let rects: Vec<Rect> = bands
        .iter()
        .enumerate()
        .map(|(i, _)| Rect {
            x: area.x,
            y: area.y + header_h + strip_h + lens[..i].iter().sum::<u16>(),
            width: area.width,
            height: lens[i],
        })
        .collect();

    let band_of = |key: Key| -> Rect {
        bands
            .iter()
            .position(|b| b.key == key)
            .map(|i| rects[i])
            .unwrap_or(Rect::ZERO)
    };

    let (overview, flow, diagnostics, connection, main) = if compact {
        let o = band_of(Key::Overview);
        let f = band_of(Key::Flow);
        let main = if f.height > 0 {
            Rect {
                x: area.x,
                y: o.y,
                width: area.width,
                height: o.height + f.height,
            }
        } else {
            o
        };
        (
            o,
            f,
            band_of(Key::Diagnostics),
            band_of(Key::Connection),
            main,
        )
    } else {
        let primary = band_of(Key::Primary);
        let diag_band = band_of(Key::Diagnostics);
        let (o, f) = split_primary(primary);
        let (d, dist) = split_diag(diag_band);
        (o, f, d, dist, primary)
    };

    Areas {
        header: Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: header_h,
        },
        strip: Rect {
            x: area.x,
            y: area.y + header_h,
            width: area.width,
            height: strip_h,
        },
        main,
        overview,
        flow,
        history: band_of(Key::History),
        diagnostics,
        connection,
        footer: Rect {
            x: area.x,
            y: area.y + area.height - footer_h,
            width: area.width,
            height: footer_h,
        },
        compact,
    }
}

/// Device list band: one row per device inside the border, and never so tall that
/// the panels below get no room.
fn strip_height(screen: &Screen, device_count: usize, height: u16) -> u16 {
    let rows: u16 = match screen {
        Screen::Dashboard | Screen::Devices => {
            if device_count <= 1 {
                0
            } else {
                (device_count.min(5) + 2) as u16
            }
        }
        Screen::Analytics => 1,
        _ => 0,
    };
    rows.min(height.saturating_sub(16))
}

fn min_sum(bands: &[Band]) -> u16 {
    bands.iter().map(|b| b.min).sum()
}

/// Give every band its minimum, then hand the spare rows to the bands that can use
/// them, highest weight first, up to each band's cap.
fn split(avail: u16, bands: &[Band]) -> Vec<u16> {
    let mut lens: Vec<u16> = bands.iter().map(|b| b.min).collect();
    let mut extra = avail.saturating_sub(lens.iter().sum::<u16>());
    if extra == 0 {
        return lens;
    }
    let mut order: Vec<usize> = (0..bands.len()).collect();
    order.sort_by_key(|&i| bands[i].weight);
    order.reverse();
    while extra > 0 {
        let mut grew = false;
        for &i in &order {
            if extra == 0 {
                break;
            }
            if lens[i] < bands[i].max {
                lens[i] += 1;
                extra -= 1;
                grew = true;
            }
        }
        if !grew {
            // All bands are capped; the chart is the one panel that reads more rows
            // as more telemetry, so it takes the tail.
            lens[order[0]] += extra;
            extra = 0;
        }
    }
    lens
}

/// Device overview gets a third of the width, bounded so its text never floats in
/// a panel wider than it can fill; power flow takes the rest for its bars.
fn split_primary(area: Rect) -> (Rect, Rect) {
    if area.width == 0 || area.height < 3 {
        return (area, Rect::ZERO);
    }
    let left = (area.width * 34 / 100).clamp(26, 44).min(area.width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left), Constraint::Min(0)])
        .split(area);
    (chunks[0], chunks[1])
}

/// The BMS panel is a text column; the distribution bars want the wider side.
fn split_diag(area: Rect) -> (Rect, Rect) {
    if area.width == 0 || area.height < 3 {
        return (area, Rect::ZERO);
    }
    let left = (area.width * 40 / 100).clamp(28, 46).min(area.width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left), Constraint::Min(0)])
        .split(area);
    (chunks[0], chunks[1])
}

/// Split a band into left/right columns. Narrow terminals give the right column
/// zero width, so the screens render a single panel.
pub fn columns(area: Rect, compact: bool) -> (Rect, Rect) {
    if compact || area.width < 52 {
        return (area, Rect::ZERO);
    }
    let left = (area.width * 40 / 100).clamp(24, 46).min(area.width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left), Constraint::Min(0)])
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
    fn the_chart_grows_with_the_terminal() {
        let small = compute(rect(100, 30), &Screen::Dashboard, 2);
        let big = compute(rect(200, 60), &Screen::Dashboard, 2);
        assert!(big.history.height > small.history.height + 6);
        assert!(big.overview.height <= 16, "{:?}", big.overview);
        assert!(big.diagnostics.height <= 15, "{:?}", big.diagnostics);
    }

    #[test]
    fn wide_terminal_pairs_the_panels_and_fits() {
        let a = compute(rect(120, 40), &Screen::Dashboard, 2);
        assert!(!a.compact);
        assert_eq!(a.header.height, 2);
        assert_eq!(a.footer.height, 1);
        assert!(a.overview.width >= 26 && a.overview.width <= 44);
        assert!(a.flow.width >= 40);
        assert!(a.overview.width + a.flow.width == 120);
        assert!(a.diagnostics.width >= 28 && a.connection.width >= 30);
        assert!(a.history.height >= 6);
        assert!(a.overview.height >= 8 && a.diagnostics.height >= 8);
    }

    #[test]
    fn narrow_terminal_stacks_the_panels() {
        let a = compute(rect(60, 30), &Screen::Dashboard, 2);
        assert!(a.compact);
        assert_eq!(a.flow.width, 60);
        assert!(a.overview.y < a.flow.y);
        assert!(a.connection.width > 0);
    }

    #[test]
    fn short_terminal_drops_the_bands_that_cannot_show_content() {
        let a = compute(rect(120, 12), &Screen::Dashboard, 2);
        assert_eq!(a.header.height, 1);
        assert_eq!(a.history.height, 0);
        assert_eq!(a.diagnostics.height, 0);
        assert!(a.overview.height > 0);
    }

    #[test]
    fn areas_never_exceed_the_frame() {
        for size in [
            (20, 5),
            (40, 10),
            (80, 20),
            (100, 30),
            (120, 40),
            (160, 50),
            (200, 60),
        ] {
            let area = rect(size.0, size.1);
            let a = compute(area, &Screen::Dashboard, 3);
            for r in [
                a.header,
                a.strip,
                a.main,
                a.overview,
                a.flow,
                a.history,
                a.diagnostics,
                a.connection,
                a.footer,
            ] {
                assert!(r.right() <= area.right(), "{r:?} in {area:?}");
                assert!(r.bottom() <= area.bottom(), "{r:?} in {area:?}");
            }
            let mut rows_used: Vec<(u16, u16)> = Vec::new();
            // No overlap between the vertical bands: in the wide layout two panels
            // share a band side by side, in the stacked layout every panel has its
            // own band.
            let band_list: Vec<Rect> = if a.compact {
                vec![
                    a.strip,
                    a.overview,
                    a.flow,
                    a.history,
                    a.diagnostics,
                    a.connection,
                ]
            } else {
                vec![a.strip, a.main, a.history, a.diagnostics]
            };
            for r in band_list {
                if r.height > 0 {
                    rows_used.push((r.y, r.bottom()));
                }
            }
            rows_used.sort();
            for w in rows_used.windows(2) {
                assert!(w[0].1 <= w[1].0, "bands overlap: {w:?} in {area:?}");
            }
        }
    }

    #[test]
    fn single_device_has_no_strip() {
        let a = compute(rect(100, 30), &Screen::Dashboard, 1);
        assert_eq!(a.strip.height, 0);
        assert!(a.overview.height >= 8);
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
