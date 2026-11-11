use std::collections::HashMap;

use serde_json::Value;

use crate::devices::registry::Family;
use crate::telemetry::models::{Battery, DeviceSnapshot, Power, Settings};
use crate::utils::units::{DECI, MILLI, TENTHOUSANDTH};

/// Merge one dotted property key into a device snapshot.
///
/// Keys are the names the device itself uses (`bms_bmsStatus.soc`, `pd.wattsInSum`),
/// taken from live `quota/all` responses. Every key, known or not, is stored in
/// `raw`, so unknown properties survive for diagnostics.
pub fn apply_key(state: &mut DeviceSnapshot, key: &str, value: &Value) {
    // A live partial block reports `cellVol: []` and `cellTemp: []` for the lists it
    // did not include in that block (verified on the DELTA 2 Max, 2026-11-10). An
    // empty array is not a measurement, so it must not erase the list the device
    // reported in an earlier block or in the REST backfill.
    if value.is_array() && value.as_array().map(|a| a.is_empty()).unwrap_or(false) {
        return;
    }
    state.raw.insert(key.to_string(), value.clone());
    let f = as_f64(value);
    // The product type identifies the model. Quota payloads carry it, so a device
    // discovered from the REST list (which reports names only) gets its family and
    // capability set as soon as the first MQTT block arrives.
    if matches!(key, "bms_bmsStatus.productType" | "pd.productType")
        && let Some(t) = f
    {
        let info = crate::devices::registry::lookup(t as u16);
        state.product_type = t as u16;
        if info.family != Family::Unknown {
            state.family = info.family;
        }
    }
    match state.family {
        Family::Delta2 | Family::River => apply_delta2(state, key, f, value),
        Family::DeltaPro => apply_prefixed(state, key, f, value),
        Family::DeltaPro3 => apply_flat(state, key, f, value),
        Family::PowerStream | Family::SmartPlug | Family::Unknown => {
            apply_generic(state, key, f, value)
        }
    }
}

/// Merge an object of properties, as an MQTT block delivers them.
pub fn merge_params(state: &mut DeviceSnapshot, params: &HashMap<String, Value>) {
    for (key, value) in params {
        apply_key(state, key, value);
    }
    derive(state);
}

/// Re-derive the normalized fields from the merged raw map, in a fixed key order.
///
/// Two sources report the same device: REST returns the whole 240-key map every
/// poll, MQTT pushes partial blocks. Merging them means re-applying every key the
/// device has ever reported, in sorted order, so the fields come out the same no
/// matter which source delivered last. Starting from empty fields is what makes the
/// order fixed: conditional fills such as `pd.soc` only apply when nothing better
/// has been seen.
pub fn renorm(state: &mut DeviceSnapshot) {
    let keys: Vec<(String, Value)> = state
        .raw
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    state.battery = Battery::default();
    state.power = Power::default();
    state.settings = Settings::default();
    for (k, v) in &keys {
        apply_key(state, k, v);
    }
    derive(state);
}

pub fn merge_json(state: &mut DeviceSnapshot, obj: &serde_json::Map<String, Value>) {
    for (key, value) in obj {
        apply_key(state, key, value);
    }
    derive(state);
}

/// Derived values that depend on more than one property. Runs after each merge so
/// the snapshot is always consistent.
pub fn derive(state: &mut DeviceSnapshot) {
    let volts = state.battery.volt_v.unwrap_or(0.0);
    if let Some(mah) = state.battery.design_cap_mah {
        state.battery.capacity_wh = Some(mah * volts / 1000.0);
    }
    if state.battery.capacity_wh.is_none()
        && let Some(mah) = state.battery.full_cap_mah
    {
        state.battery.capacity_wh = Some(mah * volts / 1000.0);
    }
    if let Some(mah) = state.battery.remain_cap_mah {
        state.battery.remaining_wh = Some(mah * volts / 1000.0);
    }
    // Charge direction: prefer the measured current sign, the device's state code is kept raw.
    if let Some(amp) = state.battery.amp_a {
        if amp > 0.05 {
            state.battery.charging = Some(true);
            state.battery.discharging = Some(false);
        } else if amp < -0.05 {
            state.battery.charging = Some(false);
            state.battery.discharging = Some(true);
        } else {
            state.battery.charging = Some(false);
            state.battery.discharging = Some(false);
        }
    }
    // Solar input is the sum of both MPPT inputs on dual-input models.
    let solar = [state.power.solar_in_w, state.power.dc_in_w]
        .into_iter()
        .flatten()
        .collect::<Vec<f64>>();
    if solar.len() == 2 {
        state.power.solar_in_w = Some(solar[0] + solar[1]);
        state.power.dc_in_w = Some(0.0);
    }
    // Ports are listed individually; the UI shows them and the aggregate is not
    // re-derived, so totals never double-count.
    let mut ports = Vec::new();
    for (key, label) in [
        ("pd.typec1Watts", "USB-C 1"),
        ("pd.typec2Watts", "USB-C 2"),
        ("pd.usb1Watts", "USB 1"),
        ("pd.usb2Watts", "USB 2"),
        ("pd.qcUsb1Watts", "QC USB 1"),
        ("pd.qcUsb2Watts", "QC USB 2"),
        ("pd.carWatts", "12 V"),
        ("pd.XT150Watts1", "XT150 1"),
        ("pd.XT150Watts2", "XT150 2"),
    ] {
        if let Some(v) = state.raw.get(key).and_then(as_f64) {
            let watts = match key {
                "pd.carWatts" | "pd.qcUsb2Watts" => v * DECI,
                _ => v,
            };
            if watts > 0.0 {
                ports.push((label.to_string(), watts));
            }
        }
    }
    state.power.ports = ports;
    state.power.usb_out_w = state.power.usb_total_w();

    // The `powGet*` dialect (DELTA Pro 3, Power Stream, Smart Plug) reports power per
    // channel; the totals are the sum of the channels the device has reported.
    if matches!(
        state.family,
        Family::DeltaPro3 | Family::PowerStream | Family::SmartPlug | Family::Unknown
    ) {
        let solar = ["powGetPvH", "powGetPvL"]
            .iter()
            .filter_map(|k| state.raw.get(*k).and_then(as_f64))
            .sum::<f64>();
        let ac_out = ["powGetAcHvOut", "powGetAcLvOut", "powGetAcLvTt30Out"]
            .iter()
            .filter_map(|k| state.raw.get(*k).and_then(as_f64))
            .sum::<f64>();
        let dc_out = ["powGet4p81", "powGet4p82", "powGet5p8"]
            .iter()
            .filter_map(|k| state.raw.get(*k).and_then(as_f64))
            .sum::<f64>();
        let usb = [
            "powGetQcusb1",
            "powGetQcusb2",
            "powGetTypec1",
            "powGetTypec2",
        ]
        .iter()
        .filter_map(|k| state.raw.get(*k).and_then(as_f64))
        .sum::<f64>();
        let present = |v: f64, had: bool| -> Option<f64> { if had { Some(v) } else { None } };
        state.power.solar_in_w = present(
            solar,
            state.raw.contains_key("powGetPvH") || state.raw.contains_key("powGetPvL"),
        );
        state.power.ac_out_w = present(
            ac_out,
            state.raw.contains_key("powGetAcHvOut") || state.raw.contains_key("powGetAcLvOut"),
        );
        state.power.dc_out_w = present(
            dc_out,
            state.raw.contains_key("powGet4p81") || state.raw.contains_key("powGet4p82"),
        );
        state.power.usb_out_w = present(
            usb,
            state.raw.contains_key("powGetQcusb1") || state.raw.contains_key("powGetTypec1"),
        );
    }
}

fn apply_delta2(state: &mut DeviceSnapshot, key: &str, f: Option<f64>, value: &Value) {
    let b = &mut state.battery;
    let p = &mut state.power;
    let s = &mut state.settings;
    match key {
        "bms_bmsStatus.soc" => b.soc = f,
        "bms_bmsStatus.f32ShowSoc" => b.soc_display = f,
        "bms_emsStatus.lcdShowSoc" | "bms_emsStatus.f32LcdShowSoc" => b.soc_display = f,
        "pd.soc" => {
            if b.soc.is_none() {
                b.soc = f;
            }
        }
        "bms_bmsStatus.amp" => b.amp_a = f.map(|v| v * MILLI),
        "bms_bmsStatus.vol" => b.volt_v = f.map(|v| v * MILLI),
        "bms_bmsStatus.tagChgAmp" => b.amp_a = b.amp_a.or(f.map(|v| v * TENTHOUSANDTH)),
        "bms_bmsStatus.cycles" => b.cycles = f,
        // Verified on a live DELTA 2 Max: `soh` and `bms_bmsInfo.soh` report 99, while
        // `realSoh` and `caleSoh` report 0 (unset placeholders). Only the first two are
        // used, and a zero is ignored so a later key cannot clobber the real value.
        "bms_bmsStatus.soh" | "bms_bmsInfo.soh" => b.soh = b.soh.or(f.filter(|v| *v > 0.0)),
        "bms_bmsStatus.temp" => b.temp_c = f,
        "bms_bmsStatus.maxCellTemp" => b.max_cell_temp_c = f,
        "bms_bmsStatus.minCellTemp" => b.min_cell_temp_c = f,
        "bms_bmsStatus.maxMosTemp" => b.max_mos_temp_c = f,
        "bms_bmsStatus.designCap" => b.design_cap_mah = f,
        "bms_bmsStatus.fullCap" => b.full_cap_mah = f,
        "bms_bmsStatus.remainCap" => b.remain_cap_mah = f,
        "bms_bmsStatus.remainTime" => b.remain_time_min = f,
        "bms_bmsStatus.chgState" => b.chg_state_raw = f,
        "bms_emsStatus.chgState" | "pd.chgDsgState" => b.chg_state_raw = b.chg_state_raw.or(f),
        // The device reports one of these depending on charge/discharge state; the
        // most recent one is what the device's own LCD shows.
        "bms_emsStatus.chgRemainTime" | "bms_emsStatus.dsgRemainTime" | "pd.remainTime" => {
            b.remain_time_min = f
        }
        "bms_emsStatus.maxChargeSoc" => s.max_charge_soc = f,
        "bms_emsStatus.minDsgSoc" => s.min_discharge_soc = f,
        "pd.minAcSoc" | "pd.minAcoutSoc" => s.min_discharge_soc = s.min_discharge_soc.or(f),
        "inv.SlowChgWatts" => s.slow_charge_watts = f,
        "inv.FastChgWatts" => s.fast_charge_watts = f,
        "inv.acChgRatedPower" => s.fast_charge_watts = s.fast_charge_watts.or(f),
        "inv.cfgAcEnabled" => s.ac_enabled = f.map(|v| v != 0.0),
        "pd.acEnabled" => s.ac_enabled = s.ac_enabled.or(f.map(|v| v != 0.0)),
        "pd.dcOutState" => s.dc_enabled = f.map(|v| v != 0.0),
        "inv.cfgAcXboost" => s.xboost = f.map(|v| v != 0.0),
        "inv.cfgAcWorkMode" => s.ac_work_mode = f,
        "pd.beepMode" => s.beep = f.map(|v| v != 0.0),
        "pd.standbyMin" => s.standby_min = f,
        "pd.pvChargePrioSet" | "pd.pvChgPrioSet" => s.pv_charge_priority = f,
        "pd.wattsInSum" => p.total_in_w = f,
        "pd.wattsOutSum" => p.total_out_w = f,
        "inv.inputWatts" => p.ac_in_w = f,
        "inv.outputWatts" => p.ac_out_w = f,
        // Verified live in the `pdStatus` MQTT block: the same AC figures also appear
        // under `pd` as `invInWatts` / `invOutWatts`. They fill the field only when the
        // `inv` block has not reported, so one measurement is never counted twice.
        "pd.invInWatts" => p.ac_in_w = p.ac_in_w.or(f),
        "pd.invOutWatts" => p.ac_out_w = p.ac_out_w.or(f),
        // Every block carries the current measurement, not an increment, so each key
        // replaces its field. Accumulating would double-count across publishes.
        "mppt.inWatts" => p.solar_in_w = f,
        "mppt.pv2InWatts" => p.dc_in_w = f,
        "mppt.inVol" => p.solar_volts = f.map(|v| v * MILLI),
        "mppt.inAmp" => p.solar_amps = f.map(|v| v * MILLI),
        "mppt.mpptTemp" => p.mppt_temp_c = f,
        "inv.outTemp" => p.inv_temp_c = f,
        "inv.acInVol" => p.ac_in_volts = f.map(|v| v * MILLI),
        "inv.acInAmp" => p.ac_in_amps = f.map(|v| v * MILLI),
        "inv.invOutVol" => p.ac_out_volts = f.map(|v| v * MILLI),
        "inv.invOutAmp" => p.ac_out_amps = f.map(|v| v * MILLI),
        // Raw frequency code, kept verbatim; no scaling is verified for this field.
        "inv.invOutFreq" => p.ac_out_freq_hz = f,
        "pd.carWatts" => p.car_out_w = f.map(|v| v * DECI),
        "pd.typec1Watts" | "pd.typec2Watts" => p.typec_w_w = f,
        // Individual USB / QC / XT150 port values are collected by `derive` into the
        // port list; the aggregate is not accumulated here.
        "pd.usb1Watts" | "pd.usb2Watts" | "pd.qcUsb1Watts" | "pd.qcUsb2Watts"
        | "pd.XT150Watts1" | "pd.XT150Watts2" => {}
        "pd.pv1ChargeWatts" | "pd.pv2ChargeWatts" => p.solar_in_w = f,
        "pd.chgPowerAC" => b.charged_kwh = Some(f.unwrap_or(0.0) / 1000.0),
        "pd.dsgPowerAC" => b.discharged_kwh = Some(f.unwrap_or(0.0) / 1000.0),
        "pd.chgSunPower" => b.solar_charged_kwh = Some(f.unwrap_or(0.0) / 1000.0),
        "bms_bmsStatus.cellVol" => b.cell_volts = array_scaled(value, MILLI),
        "bms_bmsStatus.cellTemp" => b.cell_temps = array_values(value),
        "bms_bmsStatus.productType" => {
            if let Some(v) = f {
                state.product_type = v as u16;
                state.family = Family::from_product_type(v as u16);
            }
        }
        "bms_kitInfo.watts" => state.extra_kits(value),
        _ => {}
    }
}

/// DELTA Pro / Mini: the leaf names are shared, only the prefix differs.
fn apply_prefixed(state: &mut DeviceSnapshot, key: &str, f: Option<f64>, value: &Value) {
    let leaf = key.split('.').nth(1).unwrap_or(key);
    match leaf {
        "soc" | "f32ShowSoc" => {
            if leaf == "soc" {
                state.battery.soc = f;
            } else {
                state.battery.soc_display = f;
            }
        }
        "lcdShowSoc" | "f32LcdShowSoc" => state.battery.soc_display = f,
        "amp" => state.battery.amp_a = f.map(|v| v * MILLI),
        "vol" => state.battery.volt_v = f.map(|v| v * MILLI),
        "cycles" => state.battery.cycles = f,
        "soh" => state.battery.soh = f,
        "temp" => state.battery.temp_c = f,
        "maxCellTemp" => state.battery.max_cell_temp_c = f,
        "minCellTemp" => state.battery.min_cell_temp_c = f,
        "designCap" => state.battery.design_cap_mah = f,
        "fullCap" => state.battery.full_cap_mah = f,
        "remainCap" => state.battery.remain_cap_mah = f,
        "cellVol" => state.battery.cell_volts = array_scaled(value, MILLI),
        "cellTemp" => state.battery.cell_temps = array_values(value),
        "maxChargeSoc" => state.settings.max_charge_soc = f,
        "minDsgSoc" => state.settings.min_discharge_soc = f,
        "chgRemainTime" | "dsgRemainTime" => state.battery.remain_time_min = f,
        "wattsInSum" => state.power.total_in_w = f,
        "wattsOutSum" => state.power.total_out_w = f,
        "inputWatts" if key.starts_with("inv.") => state.power.ac_in_w = f,
        "outputWatts" if key.starts_with("inv.") => state.power.ac_out_w = f,
        "inWatts" => state.power.solar_in_w = f,
        "cfgAcEnabled" => state.settings.ac_enabled = f.map(|v| v != 0.0),
        "acOutState" => {
            state.settings.dc_enabled = state.settings.dc_enabled.or(f.map(|v| v != 0.0))
        }
        _ => {}
    }
}

/// DELTA Pro 3 uses flat camelCase keys with no channel prefix.
fn apply_flat(state: &mut DeviceSnapshot, key: &str, f: Option<f64>, _value: &Value) {
    match key {
        "bmsBattSoc" | "cmsBattSoc" => {
            if key == "bmsBattSoc" {
                state.battery.soc = f;
            } else {
                state.battery.soc_display = f;
            }
        }
        "bmsBattSoh" | "cmsBattSoh" => state.battery.soh = f,
        "bmsDesignCap" => state.battery.design_cap_mah = f.map(|v| v * 1000.0),
        "cmsBattFullEnergy" => state.battery.capacity_wh = f,
        "cmsBattPowInMax" => state.settings.fast_charge_watts = f,
        "cmsBattPowOutMax" => {
            state.settings.fast_charge_watts = state.settings.fast_charge_watts.or(f)
        }
        "cmsChgDsgState" => state.battery.chg_state_raw = f,
        "cmsChgRemTime" | "cmsDsgRemTime" | "bmsChgRemTime" | "bmsDsgRemTime" => {
            state.battery.remain_time_min = f
        }
        "cmsMaxChgSoc" => state.settings.max_charge_soc = f,
        "cmsMinDsgSoc" => state.settings.min_discharge_soc = f,
        "bmsMaxCellTemp" | "bmsMaxMosTemp" => state.battery.max_cell_temp_c = f,
        "bmsMinCellTemp" | "bmsMinMosTemp" => state.battery.min_cell_temp_c = f,
        "powInSumW" => state.power.total_in_w = f,
        "powOutSumW" => state.power.total_out_w = f,
        "powGetAc" | "powGetAcIn" => state.power.ac_in_w = f,
        // Multi-channel power keys are summed from `raw` in `derive`, so repeated
        // blocks replace the channel value instead of accumulating.
        "powGetPvH" | "powGetPvL" | "powGetAcHvOut" | "powGetAcLvOut" | "powGetAcLvTt30Out"
        | "powGet4p81" | "powGet4p82" | "powGet5p8" | "powGetQcusb1" | "powGetQcusb2"
        | "powGetTypec1" | "powGetTypec2" => {}
        "powGet12v" => state.power.car_out_w = f,
        "powGetLlc" => state.power.dc_in_w = f,
        _ => {}
    }
}

/// Nothing is assumed for a device whose dialect is not known.
fn apply_generic(state: &mut DeviceSnapshot, key: &str, f: Option<f64>, _value: &Value) {
    if key == "online" {
        state.online = f.map(|v| v != 0.0).unwrap_or(true)
    }
}

/// Parse a payload value as a number. Strings, bools and numbers all appear in real
/// payloads; anything else yields `None`.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

pub fn array_scaled(value: &Value, multiplier: f64) -> Vec<f64> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(as_f64)
                .map(|v| v * multiplier)
                .collect()
        })
        .unwrap_or_default()
}

pub fn array_values(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|arr| arr.iter().filter_map(as_f64).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn snapshot(family: Family) -> DeviceSnapshot {
        let mut s = DeviceSnapshot::new("TEST", "DELTA 2 Max", 81);
        s.family = family;
        s.last_update = Utc::now();
        s
    }

    fn map(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn live_delta2max_keys_normalize() {
        let mut s = snapshot(Family::Delta2);
        merge_params(
            &mut s,
            &map(&[
                ("bms_bmsStatus.soc", Value::from(85)),
                ("bms_bmsStatus.amp", Value::from(-5183)),
                ("bms_bmsStatus.vol", Value::from(52985)),
                ("bms_bmsStatus.remainCap", Value::from(34500)),
                ("bms_bmsStatus.designCap", Value::from(4096000)),
                ("bms_bmsStatus.cycles", Value::from(64)),
                ("bms_bmsStatus.soh", Value::from(99)),
                ("bms_bmsStatus.temp", Value::from(29)),
                ("pd.wattsInSum", Value::from(0)),
                ("pd.wattsOutSum", Value::from(218)),
                ("inv.outputWatts", Value::from(180)),
                ("mppt.inWatts", Value::from(450)),
                ("pd.usb1Watts", Value::from(50)),
                ("inv.cfgAcEnabled", Value::from(1)),
            ]),
        );
        assert_eq!(s.battery.soc, Some(85.0));
        assert_eq!(s.battery.amp_a, Some(-5.183));
        assert_eq!(s.battery.volt_v, Some(52.985));
        assert_eq!(s.battery.cycles, Some(64.0));
        assert_eq!(s.battery.soh, Some(99.0));
        // 34500 mAh at 52.985 V = 1827.98 Wh
        assert!((s.battery.remaining_wh.unwrap() - 1827.98).abs() < 0.01);
        assert_eq!(s.power.total_out_w, Some(218.0));
        assert_eq!(s.power.ac_out_w, Some(180.0));
        assert_eq!(s.power.solar_in_w, Some(450.0));
        assert_eq!(s.settings.ac_enabled, Some(true));
        // Discharging current means discharging, not charging.
        assert_eq!(s.battery.discharging, Some(true));
        assert_eq!(s.battery.charging, Some(false));
    }

    #[test]
    fn unknown_keys_are_preserved() {
        let mut s = snapshot(Family::Delta2);
        merge_params(&mut s, &map(&[("pd.brandNewFlag2026", Value::from(7))]));
        assert_eq!(s.raw.get("pd.brandNewFlag2026").and_then(as_f64), Some(7.0));
    }

    #[test]
    fn an_empty_array_from_a_block_keeps_the_reported_list() {
        // The live `bmsStatus` block carries `cellVol: []` / `cellTemp: []` when the
        // block does not include those lists; the earlier full report must survive.
        let mut s = snapshot(Family::Delta2);
        merge_params(
            &mut s,
            &map(&[
                ("bms_bmsStatus.cellVol", Value::from(vec![3297, 3303])),
                ("bms_bmsStatus.cellTemp", Value::from(vec![28, 29])),
            ]),
        );
        merge_params(
            &mut s,
            &map(&[
                ("bms_bmsStatus.cellVol", Value::Array(vec![])),
                ("bms_bmsStatus.cellTemp", Value::Array(vec![])),
                ("bms_bmsStatus.soc", Value::from(98)),
            ]),
        );
        assert_eq!(
            s.battery.cell_volts.len(),
            2,
            "the empty block did not erase the cells"
        );
        assert_eq!(s.battery.cell_temps.len(), 2);
        assert_eq!(
            s.battery.soc,
            Some(98.0),
            "the block that did report is applied"
        );
    }

    #[test]
    fn partial_blocks_merge_into_one_state() {
        let mut s = snapshot(Family::Delta2);
        merge_params(&mut s, &map(&[("bms_bmsStatus.soc", Value::from(80.0))]));
        merge_params(&mut s, &map(&[("pd.wattsOutSum", Value::from(120.0))]));
        merge_params(&mut s, &map(&[("bms_bmsStatus.soc", Value::from(79.0))]));
        assert_eq!(s.battery.soc, Some(79.0));
        assert_eq!(s.power.total_out_w, Some(120.0));
        assert_eq!(s.raw.len(), 2);
    }

    #[test]
    fn null_and_string_values_do_not_panic() {
        let mut s = snapshot(Family::Delta2);
        merge_params(
            &mut s,
            &map(&[
                ("bms_bmsStatus.soc", Value::Null),
                ("pd.wattsInSum", Value::String("250".into())),
                ("pd.icoBytes", Value::from(vec![1u8, 2, 3])),
                ("inv.cfgAcEnabled", Value::Bool(true)),
            ]),
        );
        assert_eq!(s.battery.soc, None);
        assert_eq!(s.power.total_in_w, Some(250.0));
        assert_eq!(s.settings.ac_enabled, Some(true));
        assert_eq!(s.raw.len(), 4);
    }

    #[test]
    fn delta_pro_prefixes_are_shared() {
        let mut s = snapshot(Family::DeltaPro);
        merge_params(
            &mut s,
            &map(&[
                ("bmsMaster.soc", Value::from(66)),
                ("bmsMaster.amp", Value::from(1500)),
                ("bmsMaster.vol", Value::from(48000)),
                ("ems.maxChargeSoc", Value::from(85)),
            ]),
        );
        assert_eq!(s.battery.soc, Some(66.0));
        assert_eq!(s.battery.amp_a, Some(1.5));
        assert_eq!(s.settings.max_charge_soc, Some(85.0));
    }

    #[test]
    fn delta_pro3_flat_keys_normalize() {
        let mut s = snapshot(Family::DeltaPro3);
        merge_params(
            &mut s,
            &map(&[
                ("bmsBattSoc", Value::from(55)),
                ("powInSumW", Value::from(1200)),
                ("powGetPvH", Value::from(800)),
                ("powGetPvL", Value::from(400)),
                ("powGetAcHvOut", Value::from(1000)),
            ]),
        );
        assert_eq!(s.battery.soc, Some(55.0));
        assert_eq!(s.power.total_in_w, Some(1200.0));
        assert_eq!(s.power.solar_in_w, Some(1200.0));
        assert_eq!(s.power.ac_out_w, Some(1000.0));
    }

    #[test]
    fn dual_solar_inputs_sum_into_solar() {
        let mut s = snapshot(Family::Delta2);
        merge_params(
            &mut s,
            &map(&[
                ("mppt.inWatts", Value::from(300)),
                ("mppt.pv2InWatts", Value::from(200)),
            ]),
        );
        assert_eq!(s.power.solar_in_w, Some(500.0));
    }

    #[test]
    fn cell_arrays_scale_to_volts() {
        let mut s = snapshot(Family::Delta2);
        merge_params(
            &mut s,
            &map(&[("bms_bmsStatus.cellVol", Value::from(vec![3300, 3310]))]),
        );
        let volts = &s.battery.cell_volts;
        assert_eq!(volts.len(), 2);
        assert!((volts[0] - 3.30).abs() < 1e-9);
        assert!((volts[1] - 3.31).abs() < 1e-9);
    }

    #[test]
    fn the_live_240_key_map_normalizes_into_every_field() {
        // The fixture is the real `quota/all` response for the DELTA 2 Max in this
        // account, with the serial values masked.
        let raw = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../tests/fixtures/quota_all.json"
        ))
        .expect("fixture");
        let map: HashMap<String, Value> = raw.as_object().unwrap().clone().into_iter().collect();
        let mut s = snapshot(Family::Delta2);
        merge_params(&mut s, &map);
        renorm(&mut s);
        assert_eq!(s.battery.soc, Some(85.0));
        assert!((s.battery.volt_v.unwrap() - 52.985).abs() < 0.001);
        assert!((s.battery.capacity_wh.unwrap() - 2119.4).abs() < 1.0);
        assert_eq!(s.battery.soh, Some(99.0));
        assert_eq!(s.battery.cycles, Some(64.0));
        assert_eq!(
            s.battery.cell_volts.len(),
            16,
            "the cell array is scaled to volts"
        );
        assert!(s.power.ac_out_w.is_some());
        // 240 keys are the device's whole reported vocabulary.
        assert_eq!(s.raw.len(), 240);
    }

    #[test]
    fn soh_placeholders_never_clobber_the_real_value() {
        // On the live DELTA 2 Max, `realSoh` and `caleSoh` are 0 while `soh` is 99, and
        // the map iteration order is arbitrary, so both orders must give 99.
        let mut s = snapshot(Family::Delta2);
        apply_key(&mut s, "bms_bmsStatus.realSoh", &Value::from(0));
        apply_key(&mut s, "bms_bmsStatus.caleSoh", &Value::from(0));
        assert_eq!(s.battery.soh, None, "a placeholder is not a measurement");
        apply_key(&mut s, "bms_bmsStatus.soh", &Value::from(99));
        apply_key(&mut s, "bms_bmsStatus.realSoh", &Value::from(0));
        assert_eq!(s.battery.soh, Some(99.0));
    }
}
