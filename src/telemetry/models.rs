use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::devices::registry::Family;

/// Where the newest value for a device came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    None,
    Mqtt,
    Rest,
    Mock,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::None => "",
            Source::Mqtt => "MQTT",
            Source::Rest => "REST",
            Source::Mock => "MOCK",
        }
    }
}

/// Provenance of a displayed number: a real measurement, a device-reported
/// estimate, or something the app computed. The UI shows this distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provenance {
    #[default]
    Unknown,
    Measured,
    DeviceEstimate,
    Computed,
}

#[derive(Debug, Clone, Default)]
pub struct Battery {
    /// BMS state of charge, %.
    pub soc: Option<f64>,
    /// What the device shows on its own LCD, % (device-reported).
    pub soc_display: Option<f64>,
    /// Remaining energy, Wh. Derived from `remainCap` at the measured bus voltage.
    pub remaining_wh: Option<f64>,
    /// Nominal capacity, Wh.
    pub capacity_wh: Option<f64>,
    pub soh: Option<f64>,
    pub temp_c: Option<f64>,
    pub max_cell_temp_c: Option<f64>,
    pub min_cell_temp_c: Option<f64>,
    pub max_mos_temp_c: Option<f64>,
    pub amp_a: Option<f64>,
    pub volt_v: Option<f64>,
    pub cycles: Option<f64>,
    /// Time to full/empty as reported by the device, minutes.
    pub remain_time_min: Option<f64>,
    pub cell_volts: Vec<f64>,
    pub cell_temps: Vec<f64>,
    pub charging: Option<bool>,
    pub discharging: Option<bool>,
    /// Raw capacities in mAh, kept so Wh can be derived at the measured bus voltage.
    pub remain_cap_mah: Option<f64>,
    pub design_cap_mah: Option<f64>,
    pub full_cap_mah: Option<f64>,
    /// Raw BMS charge/discharge state codes, kept verbatim for diagnostics.
    pub chg_state_raw: Option<f64>,
    pub dsg_state_raw: Option<f64>,
    /// Lifetime energy counters, kWh, where the device reports them.
    pub charged_kwh: Option<f64>,
    pub discharged_kwh: Option<f64>,
    pub solar_charged_kwh: Option<f64>,
}

impl Battery {
    /// Estimated time left at the current current, computed by the app.
    /// Positive current = charging, negative = discharging.
    pub fn computed_runtime_min(&self) -> Option<f64> {
        let (Some(amp), Some(vol), Some(remaining_wh)) =
            (self.amp_a, self.volt_v, self.remaining_wh)
        else {
            return None;
        };
        if amp.abs() < 0.05 {
            return None;
        }
        let power_w = amp * vol;
        if power_w > 0.0 {
            // Charging: time to reach capacity.
            let capacity = self.capacity_wh?;
            let headroom = capacity - remaining_wh;
            if headroom <= 0.0 {
                return Some(0.0);
            }
            return Some(headroom / power_w * 60.0);
        }
        if remaining_wh <= 0.0 {
            return Some(0.0);
        }
        Some(remaining_wh / power_w.abs() * 60.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Power {
    pub total_in_w: Option<f64>,
    pub ac_in_w: Option<f64>,
    pub solar_in_w: Option<f64>,
    pub dc_in_w: Option<f64>,
    pub total_out_w: Option<f64>,
    pub ac_out_w: Option<f64>,
    pub dc_out_w: Option<f64>,
    pub usb_out_w: Option<f64>,
    pub car_out_w: Option<f64>,
    pub typec_w_w: Option<f64>,
    /// AC input electrical details, when the device reports them.
    pub ac_in_volts: Option<f64>,
    pub ac_in_amps: Option<f64>,
    pub ac_out_volts: Option<f64>,
    pub ac_out_amps: Option<f64>,
    pub ac_out_freq_hz: Option<f64>,
    /// Solar (MPPT) side details.
    pub solar_volts: Option<f64>,
    pub solar_amps: Option<f64>,
    pub mppt_temp_c: Option<f64>,
    pub inv_temp_c: Option<f64>,
    /// Individual port readings that the device reports.
    pub ports: Vec<(String, f64)>,
}

impl Power {
    /// Total of the low-voltage outputs (USB + 12 V car port + DC-DC), so the UI can
    /// show a DC/USB output figure when the device reports the ports individually.
    pub fn usb_total_w(&self) -> Option<f64> {
        let parts: Vec<f64> = [self.usb_out_w, self.typec_w_w]
            .into_iter()
            .flatten()
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.into_iter().sum())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub max_charge_soc: Option<f64>,
    pub min_discharge_soc: Option<f64>,
    pub slow_charge_watts: Option<f64>,
    pub fast_charge_watts: Option<f64>,
    pub ac_enabled: Option<bool>,
    pub dc_enabled: Option<bool>,
    pub xboost: Option<bool>,
    pub ac_work_mode: Option<f64>,
    pub beep: Option<bool>,
    pub standby_min: Option<f64>,
    pub pv_charge_priority: Option<f64>,
}

/// An accessory pack reported in `bms_kitInfo.watts`, verified shape from a live
/// DELTA 2 Max quota dump.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KitInfo {
    pub sn: String,
    pub soc: f64,
    pub cur_power_w: f64,
    pub app_state: f64,
    pub kit_type: f64,
}

/// One device, merged from MQTT partial blocks and REST snapshots.
#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    pub sn: String,
    pub product_name: String,
    pub product_type: u16,
    pub family: Family,
    pub online: bool,
    pub battery: Battery,
    pub power: Power,
    pub settings: Settings,
    /// Every property the device reported, keyed by its dotted name. Unknown
    /// properties are kept, never discarded.
    pub raw: HashMap<String, Value>,
    pub last_update: DateTime<Utc>,
    pub source: Source,
    /// Monotonic counter used to coalesce UI updates.
    pub seq: u64,
    /// Extra battery packs, when the device reports them.
    pub kits: Vec<KitInfo>,
}

impl DeviceSnapshot {
    pub fn new(sn: impl Into<String>, product_name: impl Into<String>, product_type: u16) -> Self {
        let sn = sn.into();
        let product_name = product_name.into();
        Self {
            sn,
            product_name,
            product_type,
            family: Family::from_product_type(product_type),
            online: false,
            battery: Battery::default(),
            power: Power::default(),
            settings: Settings::default(),
            raw: HashMap::new(),
            last_update: Utc::now(),
            source: Source::None,
            seq: 0,
            kits: Vec::new(),
        }
    }

    pub fn seconds_since_update(&self, now: DateTime<Utc>) -> f64 {
        (now - self.last_update).num_seconds() as f64
    }

    /// A value is stale when no update has arrived for longer than the freshness
    /// window; the UI marks it and analytics refuses to integrate over gaps.
    pub fn is_stale(&self, now: DateTime<Utc>, window_secs: f64) -> bool {
        self.seconds_since_update(now) > window_secs
    }

    pub fn masked_serial(&self) -> String {
        crate::utils::formatting::mask_serial(&self.sn)
    }

    /// Parse `bms_kitInfo.watts`, the array of accessory packs the device reports.
    pub fn extra_kits(&mut self, value: &serde_json::Value) {
        let mut kits = Vec::new();
        if let Some(arr) = value.as_array() {
            for item in arr {
                let get = |k: &str| item.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
                kits.push(KitInfo {
                    sn: item
                        .get("sn")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    soc: get("soc"),
                    cur_power_w: get("curPower"),
                    app_state: get("appState"),
                    kit_type: get("type"),
                });
            }
        }
        if !kits.is_empty() {
            self.kits = kits;
        }
    }

    /// Packs that are physically connected and reporting.
    pub fn active_kits(&self) -> impl Iterator<Item = &KitInfo> {
        self.kits
            .iter()
            .filter(|k| k.app_state != 0.0 || !k.sn.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_runtime_discharging() {
        let b = Battery {
            amp_a: Some(-2.0),
            volt_v: Some(50.0),
            remaining_wh: Some(600.0),
            ..Default::default()
        };
        // 100 W draw on 600 Wh = 6 h
        assert!((b.computed_runtime_min().unwrap() - 360.0).abs() < 0.01);
    }

    #[test]
    fn computed_runtime_charging() {
        let b = Battery {
            amp_a: Some(5.0),
            volt_v: Some(50.0),
            remaining_wh: Some(1000.0),
            capacity_wh: Some(2048.0),
            ..Default::default()
        };
        // 250 W into 1048 Wh headroom = 4.192 h
        assert!((b.computed_runtime_min().unwrap() - 251.52).abs() < 0.01);
    }

    #[test]
    fn idle_gives_no_runtime() {
        let b = Battery {
            amp_a: Some(0.0),
            volt_v: Some(50.0),
            remaining_wh: Some(500.0),
            ..Default::default()
        };
        assert_eq!(b.computed_runtime_min(), None);
    }

    #[test]
    fn new_snapshot_is_offline_with_no_data() {
        let d = DeviceSnapshot::new("SN", "DELTA 2 Max", 81);
        assert!(!d.online);
        assert_eq!(d.source, Source::None);
        assert_eq!(d.family, Family::Delta2);
    }

    #[test]
    fn source_labels() {
        assert_eq!(Source::Mqtt.label(), "MQTT");
        assert_eq!(Source::Mock.label(), "MOCK");
    }
}
