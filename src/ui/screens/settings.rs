use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::text::Line;

use crate::app::App;
use crate::ui::layout::Areas;
use crate::ui::screens::{kv, section};
use crate::ui::snapshot::Snapshot;

/// Settings screen. Values are shown from the loaded config; editing keys on the
/// screen is a later milestone, and the screen says so.
pub fn draw(app: &App, snap: &Snapshot, frame: &mut Frame, areas: Areas) {
    let cfg = &app.config;
    let (left, right) = crate::ui::layout::columns(areas.main, areas.compact);

    let api_lines = vec![
        kv(app, "region", format!("{:?}", cfg.api.region), false),
        kv(app, "base url", cfg.api.effective_base_url(), false),
        kv(
            app,
            "refresh",
            format!("{} s", cfg.api.refresh_interval_seconds),
            false,
        ),
        kv(
            app,
            "timeout",
            format!("{} s", cfg.api.timeout_seconds),
            false,
        ),
        kv(
            app,
            "controls",
            if cfg.controls.enabled {
                "ENABLED"
            } else {
                "read-only (default)"
            }
            .to_string(),
            cfg.controls.enabled,
        ),
    ];
    section(app, frame, left, "API", api_lines);

    let ui_lines = vec![
        kv(app, "fps", cfg.ui.fps.to_string(), false),
        kv(app, "colour mode", format!("{:?}", app.theme.mode), false),
        kv(
            app,
            "unicode glyphs",
            if app.theme.unicode {
                "yes"
            } else {
                "ascii fallback"
            }
            .to_string(),
            false,
        ),
        kv(
            app,
            "serial display",
            if cfg.ui.show_serial { "raw" } else { "masked" }.to_string(),
            false,
        ),
        kv(
            app,
            "temperature",
            format!("{:?}", cfg.ui.temperature_unit),
            false,
        ),
        // Device commands are gated by this switch: with it off, no key sends one.
        kv(
            app,
            "controls",
            if cfg.controls.enabled {
                "on: c = pick, left/right = value, enter/y = confirm, n/esc = cancel".to_string()
            } else {
                "off: set [controls] enabled = true to send device commands".to_string()
            },
            false,
        ),
        kv(
            app,
            "config file",
            cfg.source_path.display().to_string(),
            false,
        ),
        // Keys change these for the running session; the file is not written.
        kv(
            app,
            "changes",
            format!(
                "f = fps, u = temp unit, i = serial display (this session, {cfg_path})",
                cfg_path = cfg.source_path.display()
            ),
            false,
        ),
    ];
    if right.width > 0 {
        let (top, bottom) =
            crate::ui::layout::rows(right, Constraint::Fill(1), Constraint::Fill(1));
        section(app, frame, top, "UI", ui_lines);
        let history_lines = vec![
            kv(
                app,
                "history",
                if cfg.history.enabled { "on" } else { "off" }.to_string(),
                false,
            ),
            kv(
                app,
                "retention",
                format!("{} days", cfg.history.retention_days),
                false,
            ),
            kv(
                app,
                "sample every",
                format!("{} s", cfg.history.sample_interval_seconds),
                false,
            ),
            kv(
                app,
                "memory window",
                format!("{} samples", cfg.history.memory_samples),
                false,
            ),
            kv(
                app,
                "battery low",
                format!("{}%", cfg.alerts.battery_low_percent),
                false,
            ),
            kv(
                app,
                "battery critical",
                format!("{}%", cfg.alerts.battery_critical_percent),
                false,
            ),
            kv(
                app,
                "high temp",
                format!("{:.0} °C", cfg.alerts.high_temperature_c),
                false,
            ),
            kv(
                app,
                "output limit",
                format!("{:.0} W", cfg.alerts.high_output_w),
                false,
            ),
            kv(
                app,
                "offline notices",
                if cfg.alerts.notify_disconnected {
                    "on"
                } else {
                    "off"
                }
                .to_string(),
                false,
            ),
            kv(
                app,
                "mqtt",
                if cfg.mqtt.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
                .to_string(),
                false,
            ),
        ];
        section(app, frame, bottom, "HISTORY AND ALERTS", history_lines);
    }

    let note = Line::from("");
    let _ = note;
    crate::ui::widgets::power_graph::history(app, snap, frame, areas.history);
}
