use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Everything the user can tune. All sections have defaults, so a config file may
/// mention only the section the user cares about.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub mqtt: MqttConfig,
    pub ui: UiConfig,
    pub history: HistoryConfig,
    pub alerts: AlertConfig,
    pub analytics: AnalyticsConfig,
    pub controls: ControlConfig,
    pub update: UpdateConfig,
    /// Remapped keys: `action_name = "key"`, e.g. `quit = "ctrl+q"`.
    pub keys: std::collections::HashMap<String, String>,
    /// Where this config was loaded from; not part of the file itself.
    #[serde(skip, default)]
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Region {
    EU,
    AP,
    US,
    AS,
}

impl Region {
    /// Verified from live probes: `api-e.ecoflow.com` answered signed requests for a
    /// EU account; `api.ecoflow.com` answered as well. `api-ap` did not resolve.
    pub fn base_url(&self) -> &'static str {
        match self {
            Region::EU => "https://api-e.ecoflow.com",
            Region::AP => "https://api-ap.ecoflow.com",
            Region::US => "https://api.ecoflow.com",
            Region::AS => "https://api-as.ecoflow.com",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    pub region: Region,
    /// REST polling cadence. Used for initial sync and as the MQTT fallback.
    pub refresh_interval_seconds: u64,
    /// Escape hatch when a region host does not serve the account.
    pub base_url_override: Option<String>,
    /// Request timeout, seconds.
    pub timeout_seconds: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            region: Region::EU,
            refresh_interval_seconds: 30,
            base_url_override: None,
            timeout_seconds: 10,
        }
    }
}

impl ApiConfig {
    pub fn effective_base_url(&self) -> String {
        self.base_url_override
            .clone()
            .unwrap_or_else(|| self.region.base_url().to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MqttConfig {
    pub enabled: bool,
    pub reconnect: bool,
    /// MQTT keepalive. Implementations in the wild use 15–60 s.
    pub keepalive_secs: u16,
    /// How often to publish `latestQuotas` so the device keeps pushing updates.
    /// Devices throttle their pushes when nobody polls.
    pub refresh_interval_seconds: u64,
    /// Fixed client id. The broker allows only ~10 unique ids per day per key.
    pub client_id: Option<String>,
    pub host_override: Option<String>,
    /// Force TLS on/off. `None` uses what the certification endpoint reports.
    /// Setting this to false is for a local broker in tests, never for the real one.
    pub tls_override: Option<bool>,
    pub verify_broker_cert: bool,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reconnect: true,
            keepalive_secs: 30,
            refresh_interval_seconds: 30,
            client_id: None,
            host_override: None,
            tls_override: None,
            verify_broker_cert: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub theme: Theme,
    pub color_mode: ColorMode,
    pub fps: u32,
    pub show_serial: bool,
    pub temperature_unit: TempUnit,
    /// `watts` renders 1420 W, `auto` renders 1.4 kW above 1 kW.
    pub power_unit: PowerUnit,
    pub show_mock_badge: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            color_mode: ColorMode::Auto,
            fps: 30,
            show_serial: false,
            temperature_unit: TempUnit::Celsius,
            power_unit: PowerUnit::Auto,
            show_mock_badge: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum ColorMode {
    /// Use RGB when the terminal looks capable, 256 colours otherwise.
    #[default]
    Auto,
    TrueColor,
    Ansi256,
    Basic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum TempUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum PowerUnit {
    #[default]
    Auto,
    Watts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub retention_days: u32,
    pub sample_interval_seconds: u64,
    /// In-memory sparkline window, samples per device.
    pub memory_samples: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
            // 5 seconds: a station whose power changes is charted at that resolution,
            // and the 24-hour memory window is 720 samples.
            sample_interval_seconds: 5,
            memory_samples: 720,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertConfig {
    pub battery_low_percent: u8,
    pub battery_critical_percent: u8,
    pub high_temperature_c: f64,
    /// Alert when a device's measured total output power reaches this.
    pub high_output_w: f64,
    pub notify_disconnected: bool,
    pub cooldown_seconds: u64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            battery_low_percent: 20,
            battery_critical_percent: 10,
            high_temperature_c: 45.0,
            high_output_w: 2000.0,
            notify_disconnected: true,
            cooldown_seconds: 600,
        }
    }
}

/// Analytics tuning. A tariff is the user's own number, not a figure the app
/// claims to know, so it defaults to 0 (cost is not shown until it is set).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnalyticsConfig {
    /// Currency per kWh, for the cost line. 0.0 shows no cost.
    pub tariff_per_kwh: f64,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            tariff_per_kwh: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ControlConfig {
    /// Device controls are off by default. They must be explicitly enabled.
    pub enabled: bool,
}

/// The launch update check. Mock runs skip it; `--no-update` forces it off for
/// one run, whatever this says.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateConfig {
    /// Off means no GitHub request, no download, nothing installed.
    pub enabled: bool,
    /// The fork escape hatch: the repo the latest-release query is aimed at.
    pub repo: String,
    /// Request timeout, seconds.
    pub timeout_seconds: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: "olexanderboychuk/wattdeck".to_string(),
            timeout_seconds: 10,
        }
    }
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=60).contains(&self.ui.fps),
            "ui.fps must be between 1 and 60, got {}",
            self.ui.fps
        );
        anyhow::ensure!(
            self.api.refresh_interval_seconds >= 5,
            "api.refresh_interval_seconds must be at least 5 s (API rate limits), got {}",
            self.api.refresh_interval_seconds
        );
        anyhow::ensure!(
            self.history.sample_interval_seconds >= 1,
            "history.sample_interval_seconds must be at least 1"
        );
        anyhow::ensure!(
            self.analytics.tariff_per_kwh >= 0.0,
            "analytics.tariff_per_kwh must not be negative, got {}",
            self.analytics.tariff_per_kwh
        );
        anyhow::ensure!(
            self.ui.fps >= 1 && self.ui.fps <= 60,
            "ui.fps must be between 1 and 60, got {}",
            self.ui.fps
        );
        anyhow::ensure!(
            self.history.retention_days >= 1,
            "history.retention_days must be at least 1"
        );
        anyhow::ensure!(
            self.history.memory_samples >= 8,
            "history.memory_samples must be at least 8"
        );
        anyhow::ensure!(
            self.alerts.battery_critical_percent <= self.alerts.battery_low_percent,
            "alerts.battery_critical_percent ({}%) must not exceed battery_low_percent ({}%)",
            self.alerts.battery_critical_percent,
            self.alerts.battery_low_percent
        );
        anyhow::ensure!(
            self.mqtt.refresh_interval_seconds >= 10,
            "mqtt.refresh_interval_seconds must be at least 10 s"
        );
        anyhow::ensure!(
            self.update.timeout_seconds >= 1 && self.update.timeout_seconds <= 60,
            "update.timeout_seconds must be between 1 and 60, got {}",
            self.update.timeout_seconds
        );
        anyhow::ensure!(
            !self.update.repo.trim().is_empty(),
            "update.repo must name a repo"
        );
        Ok(())
    }

    pub fn tick_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis((1000u32 / self.ui.fps.max(1)) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_have_hosts() {
        assert_eq!(Region::EU.base_url(), "https://api-e.ecoflow.com");
        assert_eq!(Region::US.base_url(), "https://api.ecoflow.com");
    }

    #[test]
    fn base_url_override_wins() {
        let cfg = ApiConfig {
            base_url_override: Some("https://proxy.example".into()),
            ..ApiConfig::default()
        };
        assert_eq!(cfg.effective_base_url(), "https://proxy.example");
    }

    #[test]
    fn controls_are_off_by_default() {
        assert!(!Config::default().controls.enabled);
    }

    #[test]
    fn critical_above_low_is_rejected() {
        let mut cfg = Config::default();
        cfg.alerts.battery_critical_percent = 50;
        cfg.alerts.battery_low_percent = 20;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn subsecond_refresh_is_rejected() {
        let mut cfg = Config::default();
        cfg.api.refresh_interval_seconds = 1;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn tick_matches_fps() {
        let cfg = Config::default();
        assert_eq!(cfg.tick_interval(), std::time::Duration::from_millis(33));
    }

    #[test]
    fn update_defaults_check_every_launch() {
        let cfg = Config::default();
        assert!(cfg.update.enabled, "the launch check is on by default");
        assert_eq!(cfg.update.repo, "olexanderboychuk/wattdeck");
        assert_eq!(cfg.update.timeout_seconds, 10);
    }

    #[test]
    fn update_timeout_outside_1_to_60_is_refused() {
        let mut cfg = Config::default();
        cfg.update.timeout_seconds = 0;
        assert!(cfg.validate().is_err(), "0 s must be refused");
        cfg.update.timeout_seconds = 61;
        assert!(cfg.validate().is_err(), "61 s must be refused");
        cfg.update.timeout_seconds = 10;
        cfg.update.repo = "  ".to_string();
        assert!(cfg.validate().is_err(), "an empty repo must be refused");
    }

    #[test]
    fn config_toml_without_an_update_section_uses_defaults() {
        let cfg: Config = toml::from_str("[api]\nregion = \"US\"\n").unwrap();
        assert!(
            cfg.update.enabled,
            "a config file may name no [update] section"
        );
    }
}
