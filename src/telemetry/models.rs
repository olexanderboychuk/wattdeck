use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::devices::capabilities::DeviceCapabilities;

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

/// A battery pack, as far as the device reports it.
///
/// `soc` is the BMS state of charge; `soc_display` is what the device shows on its
/// own LCD. They are two different numbers, and the user reads the LCD.
/// `remaining_wh` is derived from `remainCap` (mAh) at the measured bus voltage:
/// 6000 mAh at 49 V is 294 Wh, not 6.0 Wh, which is the reading a unit bug
/// produces. The device does not report the pack capacity on this family;
/// `capacity_wh` comes from configuration, and the UI says so.
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
    /// Runtime counters, minutes, kept one per device key. Each key means something
    /// different: `bms_bmsStatus.remainTime` is signed (positive = charging),
    /// `bms_emsStatus.chgRemainTime` is time to full, `bms_emsStatus.dsgRemainTime`
    /// is time to empty, `pd.remainTime` is what the LCD shows. Merging them into one
    /// field is what makes a charging station read as "99 h left".
    pub bms_remain_min: Option<f64>,
    pub chg_remain_min: Option<f64>,
    pub dsg_remain_min: Option<f64>,
    pub lcd_remain_min: Option<f64>,
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
    /// `pd.chgDsgState`, the state code the device shows on its screen:
    /// 0 = discharging, 1 = charging (verified against the EcoFlow app).
    pub pd_chg_dsg_raw: Option<f64>,
    /// Lifetime energy counters, kWh, where the device reports them.
    pub charged_kwh: Option<f64>,
    pub discharged_kwh: Option<f64>,
    pub solar_charged_kwh: Option<f64>,
}

impl Battery {
    /// Measured battery power, W, signed: positive = charging. `None` when the
    /// ammeter is inside the noise band, which is how a station at rest reports.
    pub fn bus_power_w(&self) -> Option<f64> {
        self.amp_a
            .zip(self.volt_v)
            .filter(|(a, _)| a.abs() >= 0.05)
            .map(|(a, v)| a * v)
    }

    /// Cells whose reading sits in the band a lithium cell can report. A 0 that the
    /// device sends for a slot it is not measuring is not a cell, so counting raw
    /// array length overstates the pack; the band is what makes the count honest.
    pub fn valid_cells(&self) -> usize {
        self.cell_volts
            .iter()
            .filter(|v| (2.0..=4.5).contains(*v))
            .count()
    }

    /// The lowest and highest cell the device measured. A 0 the device sends for a
    /// slot it is not measuring is not a cell, so the spread never counts an empty
    /// slot as a discharged cell.
    pub fn cell_range(&self) -> Option<(f64, f64)> {
        let mut range: Option<(f64, f64)> = None;
        for v in self.cell_volts.iter().filter(|v| (2.0..=4.5).contains(*v)) {
            range = Some(match range {
                Some((lo, hi)) => (lo.min(*v), hi.max(*v)),
                None => (*v, *v),
            });
        }
        range
    }

    /// Spread across the cells the device measured, mV.
    pub fn cell_delta_mv(&self) -> Option<f64> {
        let (lo, hi) = self.cell_range()?;
        if hi > lo {
            Some((hi - lo) * 1000.0)
        } else {
            None
        }
    }
}

/// Voltage and current readings, in the units the device reports them.
///
/// `ac_out_va` / `ac_in_va` are apparent power (volts × amps), a different physical
/// quantity from active power (W). They are never added to a watt figure.
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
    /// Apparent power, VA, derived from the V/A pair the inverter reports.
    pub ac_out_va: Option<f64>,
    pub ac_in_va: Option<f64>,
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
    /// Every power channel the UI can show, tagged by direction and by whether the
    /// model is known to report it. Three states stay distinct: `supported = false`
    /// is a channel this model does not have, `value = None` is a channel the device
    /// has not reported in this update, `Some(0.0)` is a measured zero.
    pub fn channels(&self, caps: DeviceCapabilities) -> Vec<Channel> {
        [
            ("solar in", self.solar_in_w, caps.solar_input, true),
            ("ac in", self.ac_in_w, caps.ac_input, true),
            ("dc in", self.dc_in_w, caps.dc_input, true),
            ("ac out", self.ac_out_w, caps.ac_output, false),
            ("dc out", self.dc_out_w, caps.dc_output, false),
            ("usb out", self.usb_out_w, caps.usb_output, false),
            ("12 V out", self.car_out_w, caps.dc_output, false),
        ]
        .into_iter()
        .map(|(label, value, supported, input)| Channel {
            label,
            value,
            supported,
            input,
        })
        .collect()
    }

    /// The one normalized total every panel uses: the device's own total when it
    /// reports one, otherwise the sum of the channels it did report, tagged so the
    /// UI can say where the number came from. A device that reported nothing is
    /// `None`, never a fabricated zero.
    pub fn totals(&self, caps: DeviceCapabilities) -> Totals {
        let ch = self.channels(caps);
        let sum = |input: bool| -> Vec<f64> {
            ch.iter()
                .filter(|c| c.input == input)
                .filter_map(|c| c.value)
                .collect()
        };
        let pick = |device: Option<f64>, parts: Vec<f64>| -> (Option<f64>, Provenance) {
            if let Some(v) = device {
                (Some(v), Provenance::Measured)
            } else if !parts.is_empty() {
                (Some(parts.iter().sum()), Provenance::Computed)
            } else {
                (None, Provenance::Unknown)
            }
        };
        let (in_w, in_src) = pick(self.total_in_w, sum(true));
        let (out_w, out_src) = pick(self.total_out_w, sum(false));
        Totals {
            in_w,
            in_src,
            out_w,
            out_src,
        }
    }
}

/// One power channel as the dashboard shows it.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    pub label: &'static str,
    pub value: Option<f64>,
    pub supported: bool,
    pub input: bool,
}

/// Normalized input/output power, with where each number came from.
#[derive(Debug, Clone, Copy, Default)]
pub struct Totals {
    pub in_w: Option<f64>,
    pub in_src: Provenance,
    pub out_w: Option<f64>,
    pub out_src: Provenance,
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

/// How a device is doing, as far as the app can tell. The cloud list and the live
/// feed answer different questions, so one state is the union of both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceState {
    #[default]
    /// Nothing has ever arrived for this device.
    Unknown,
    /// The device is listed and the live feed is delivering.
    Online,
    /// Listed, but the last measurement is older than the freshness window.
    Stale,
    /// The cloud says the device exists; no live feed has arrived yet.
    Connecting,
    /// The cloud says the device is not reporting.
    Offline,
    /// The cloud refused to serve this device (the account is not entitled to it).
    Unsupported,
    /// The credentials were rejected.
    AuthError,
}

impl DeviceState {
    /// Plain words, for the dashboard.
    pub fn label(self) -> &'static str {
        match self {
            DeviceState::Unknown => "no data",
            DeviceState::Online => "online",
            DeviceState::Stale => "stale",
            DeviceState::Connecting => "connecting",
            DeviceState::Offline => "offline",
            DeviceState::Unsupported => "not supported",
            DeviceState::AuthError => "sign-in failed",
        }
    }
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
    /// The last time a live feed arrived for this device.
    pub last_mqtt: Option<DateTime<Utc>>,
    /// The last time the cloud answered for this device.
    pub last_rest: Option<DateTime<Utc>>,
    /// The cloud's refusal, if it refused: the code and its words, verbatim.
    pub last_refusal: Option<String>,
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
            last_mqtt: None,
            last_rest: None,
            last_refusal: None,
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

    /// The state the app can defend: what the cloud says, what the live feed says,
    /// and how long ago the last measurement arrived.
    pub fn state(&self, now: DateTime<Utc>, stale_window_secs: f64) -> DeviceState {
        if let Some(refusal) = &self.last_refusal {
            if refusal.contains("authoriz")
                || refusal.contains("401")
                || refusal.contains("token")
                || refusal.contains("permis")
            {
                return DeviceState::AuthError;
            }
            return DeviceState::Unsupported;
        }
        if !self.online {
            // Listed, and nothing has ever arrived: the live feed has not reached it
            // yet. Once data arrived and the cloud says it is gone, it is offline.
            return if self.last_mqtt.is_none() && self.last_rest.is_none() && self.raw.is_empty() {
                DeviceState::Connecting
            } else {
                DeviceState::Offline
            };
        }
        if self.is_stale(now, stale_window_secs) {
            return DeviceState::Stale;
        }
        if self.raw.is_empty() {
            DeviceState::Connecting
        } else {
            DeviceState::Online
        }
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
    fn cells_outside_the_band_are_not_counted() {
        let b = Battery {
            cell_volts: vec![3.42, 3.44, 3.41, 0.0, 3.43, 0.0],
            ..Default::default()
        };
        assert_eq!(b.valid_cells(), 4, "the two zero slots are not cells");
        assert!((b.cell_delta_mv().unwrap() - 30.0).abs() < 0.5);
    }

    #[test]
    fn bus_power_is_signed_and_ignores_the_noise_band() {
        let charging = Battery {
            amp_a: Some(5.0),
            volt_v: Some(50.0),
            ..Default::default()
        };
        assert_eq!(charging.bus_power_w(), Some(250.0));
        let at_rest = Battery {
            amp_a: Some(0.02),
            volt_v: Some(50.0),
            ..Default::default()
        };
        assert_eq!(
            at_rest.bus_power_w(),
            None,
            "a near-zero ammeter is not a flow"
        );
    }

    #[test]
    fn new_snapshot_is_offline_with_no_data() {
        let d = DeviceSnapshot::new("SN", "DELTA 2 Max", 81);
        assert!(!d.online);
        assert_eq!(d.source, Source::None);
        assert_eq!(d.family, Family::Delta2);
        assert_eq!(d.state(Utc::now(), 120.0), DeviceState::Connecting);
    }

    #[test]
    fn a_cloud_refusal_is_a_device_problem_not_a_stale_feed() {
        let mut d = DeviceSnapshot::new("SN", "RIVER mini Plus", 58);
        d.online = true;
        d.last_refusal = Some("code 1006: device not registered".to_string());
        assert_eq!(
            d.state(Utc::now(), 120.0),
            DeviceState::Unsupported,
            "the refusal is the reason there is no telemetry"
        );
        d.last_refusal = Some("code 5002: unauthorized".to_string());
        assert_eq!(d.state(Utc::now(), 120.0), DeviceState::AuthError);
    }

    #[test]
    fn source_labels() {
        assert_eq!(Source::Mqtt.label(), "MQTT");
        assert_eq!(Source::Mock.label(), "MOCK");
    }
}
