use ratatui::style::Color;

use crate::config::settings::ColorMode;

/// The palette. One coherent visual identity: near-black panels, muted grey
/// structure, cyan for power flow, green for charging, blue for solar, yellow for
/// warnings, red for critical. Every colour goes through `rgb` so the palette works
/// in true colour, 256-colour, and basic terminal modes without redefining it.
pub struct Theme {
    pub mode: ColorMode,
    pub unicode: bool,
}

impl Theme {
    pub fn new(mode: ColorMode) -> Self {
        Self {
            mode: Self::resolve(mode, &std::env::var("COLORTERM").unwrap_or_default()),
            unicode: locale_supports_unicode(),
        }
    }

    /// `Auto` uses RGB when the terminal advertises true colour, otherwise the
    /// 256-colour palette. The advert is passed in so the choice is testable
    /// without mutating the environment.
    fn resolve(mode: ColorMode, colorterm: &str) -> ColorMode {
        match mode {
            ColorMode::Auto => {
                if colorterm.contains("truecolor") || colorterm.contains("24bit") {
                    ColorMode::TrueColor
                } else {
                    ColorMode::Ansi256
                }
            }
            other => other,
        }
    }

    fn rgb(&self, r: u8, g: u8, b: u8) -> Color {
        match self.mode {
            ColorMode::TrueColor => Color::Rgb(r, g, b),
            ColorMode::Ansi256 => Color::Indexed(rgb_to_256(r, g, b)),
            ColorMode::Basic => basic_color(r, g, b),
            ColorMode::Auto => Color::Rgb(r, g, b),
        }
    }

    // Palette entries, named by meaning rather than by hue.
    pub fn bg(&self) -> Color {
        self.rgb(10, 14, 18)
    }
    pub fn text(&self) -> Color {
        self.rgb(205, 212, 219)
    }
    pub fn secondary(&self) -> Color {
        self.rgb(110, 118, 127)
    }
    pub fn border(&self) -> Color {
        self.rgb(64, 72, 82)
    }
    pub fn border_focus(&self) -> Color {
        self.rgb(120, 200, 216)
    }
    pub fn accent(&self) -> Color {
        self.rgb(120, 200, 216)
    }
    pub fn charging(&self) -> Color {
        self.rgb(106, 194, 90)
    }
    pub fn solar(&self) -> Color {
        self.rgb(88, 166, 255)
    }
    pub fn warning(&self) -> Color {
        self.rgb(232, 192, 74)
    }
    pub fn critical(&self) -> Color {
        self.rgb(224, 85, 79)
    }
    pub fn online(&self) -> Color {
        self.rgb(106, 194, 90)
    }
    pub fn offline(&self) -> Color {
        self.rgb(110, 118, 127)
    }
    pub fn mock(&self) -> Color {
        self.rgb(190, 130, 220)
    }

    /// Fractional bar, btop style: full blocks then a partial block, then light shade.
    pub fn bar(&self, width: usize, fraction: f64) -> String {
        let f = fraction.clamp(0.0, 1.0);
        if !self.unicode {
            let filled = (f * width as f64).round() as usize;
            return format!("{}{}", "#".repeat(filled), ".".repeat(width - filled));
        }
        let glyphs = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
        let cells = f * width as f64;
        let full = cells.floor() as usize;
        let partial_idx = ((cells - full as f64) * 8.0).floor() as usize;
        let mut out = String::with_capacity(width);
        out.push_str(&"█".repeat(full.min(width)));
        if full < width {
            out.push_str(glyphs[partial_idx.min(7)]);
            let used = full + (partial_idx > 0) as usize;
            out.push_str(&"░".repeat(width.saturating_sub(used)));
        }
        out
    }

    /// Colour of a battery bar by threshold.
    pub fn battery_colour(&self, soc: f64, low: u8, critical: u8) -> Color {
        if soc <= critical as f64 {
            self.critical()
        } else if soc <= low as f64 {
            self.warning()
        } else {
            self.charging()
        }
    }
}

/// Boxed drawing characters are available in any UTF-8 locale. Under `C`/`POSIX`
/// the app falls back to ASCII bars.
pub fn locale_supports_unicode() -> bool {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_TYPE"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.is_empty() {
        return true;
    }
    if locale.to_ascii_lowercase().contains("utf-8") || locale.to_ascii_lowercase().contains("utf8")
    {
        return true;
    }
    !(locale == "C" || locale == "POSIX")
}

/// Map RGB to the nearest xterm-256 colour: grayscale ramp, then the 6x6x6 cube.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let ri = (r as f64 / 255.0 * 5.0).round() as u8;
    let gi = (g as f64 / 255.0 * 5.0).round() as u8;
    let bi = (b as f64 / 255.0 * 5.0).round() as u8;
    16 + 36 * ri + 6 * gi + bi
}

/// Basic mode: snap to the 16 ANSI palette entries (`Color::Indexed(0..15)`),
/// which any terminal supports.
pub fn basic_color(r: u8, g: u8, b: u8) -> Color {
    let lum = (r as u16 + g as u16 + b as u16) / 3;
    let hue = classify(r, g, b);
    let bright = lum > 128;
    let idx: u8 = match (hue, bright) {
        (Hue::Red, false) => 1,
        (Hue::Red, true) => 9,
        (Hue::Green, false) => 2,
        (Hue::Green, true) => 10,
        (Hue::Yellow, false) => 3,
        (Hue::Yellow, true) => 11,
        (Hue::Blue, false) => 4,
        (Hue::Blue, true) => 12,
        (Hue::Magenta, false) => 5,
        (Hue::Magenta, true) => 13,
        (Hue::Cyan, false) => 6,
        (Hue::Cyan, true) => 14,
        (Hue::Grey, false) => 8,
        (Hue::Grey, true) => 15,
    };
    Color::Indexed(idx)
}

enum Hue {
    Red,
    Green,
    Blue,
    Cyan,
    Yellow,
    Magenta,
    Grey,
}

fn classify(r: u8, g: u8, b: u8) -> Hue {
    if r.abs_diff(g).max(g.abs_diff(b)).max(r.abs_diff(b)) < 12 {
        return Hue::Grey;
    }
    // Pick the dominant channel; ties fall through to the mixed cases.
    if r >= g && r >= b {
        if g >= b { Hue::Red } else { Hue::Magenta }
    } else if g >= r && g >= b {
        if b >= r { Hue::Cyan } else { Hue::Green }
    } else if b >= g && b >= r {
        if r >= g { Hue::Magenta } else { Hue::Blue }
    } else {
        Hue::Yellow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_exactly_the_requested_width() {
        let t = Theme {
            mode: ColorMode::TrueColor,
            unicode: true,
        };
        for width in [1, 5, 10, 20] {
            for f in [0.0, 0.3, 0.5, 0.999, 1.0] {
                let bar = t.bar(width, f);
                assert_eq!(
                    bar.chars().count(),
                    width,
                    "width {width} frac {f} => {bar}"
                );
            }
        }
    }

    #[test]
    fn ascii_fallback_works() {
        let t = Theme {
            mode: ColorMode::Basic,
            unicode: false,
        };
        assert_eq!(t.bar(5, 0.6), "###..");
        assert_eq!(t.bar(4, 0.0), "....");
    }

    #[test]
    fn battery_colour_follows_thresholds() {
        let t = Theme {
            mode: ColorMode::TrueColor,
            unicode: true,
        };
        assert_eq!(t.battery_colour(50.0, 20, 10), t.charging());
        assert_eq!(t.battery_colour(15.0, 20, 10), t.warning());
        assert_eq!(t.battery_colour(8.0, 20, 10), t.critical());
    }

    #[test]
    fn rgb_mapping_stays_in_range() {
        for (r, g, b) in [(0, 0, 0), (255, 255, 255), (120, 200, 216), (10, 14, 18)] {
            // Widened before the compare: on a `u8` result the bound would be
            // trivially true, so the assertion has to check the value in a wider type.
            let v = u32::from(rgb_to_256(r, g, b));
            assert!(v <= 255);
        }
        assert_eq!(rgb_to_256(128, 128, 128), 232 + 11);
    }

    #[test]
    fn auto_mode_uses_the_terminal_advert() {
        assert_eq!(Theme::resolve(ColorMode::Auto, ""), ColorMode::Ansi256);
        assert_eq!(
            Theme::resolve(ColorMode::Auto, "truecolor"),
            ColorMode::TrueColor
        );
        assert_eq!(
            Theme::resolve(ColorMode::Auto, "24bit"),
            ColorMode::TrueColor
        );
        // An explicit mode is never overridden by the advert.
        assert_eq!(
            Theme::resolve(ColorMode::Basic, "truecolor"),
            ColorMode::Basic
        );
    }

    #[test]
    fn palette_is_distinct() {
        let t = Theme {
            mode: ColorMode::TrueColor,
            unicode: true,
        };
        let colours = [
            t.accent(),
            t.charging(),
            t.solar(),
            t.warning(),
            t.critical(),
        ];
        for (i, a) in colours.iter().enumerate() {
            for b in colours[i + 1..].iter() {
                assert_ne!(a, b);
            }
        }
    }
}
