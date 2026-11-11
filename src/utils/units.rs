//! Unit conversions for EcoFlow raw payloads.
//!
//! Raw device values arrive as scaled integers. The factors below come from the
//! real property set of a DELTA 2 Max (productType 81) read from the live
//! `iot-open/sign/device/quota/all` endpoint, cross-checked with the
//! `foxthefox/ioBroker.ecoflow-mqtt` multiplier table. Internally everything is
//! stored in SI base units: W, Wh, V, A, °C, %.

pub const MILLI: f64 = 1.0e-3;
pub const DECI: f64 = 0.1;
pub const TENTHOUSANDTH: f64 = 1.0e-4;

pub const W_PER_KW: f64 = 1000.0;
pub const WH_PER_KWH: f64 = 1000.0;

pub const MV_TO_V: f64 = MILLI;
pub const MA_TO_A: f64 = MILLI;
pub const MAH_TO_WH: f64 = MILLI; // multiplied by volts by the caller

pub const SECONDS_PER_MINUTE: f64 = 60.0;

/// Scale a raw payload value by a multiplier.
pub fn scale(raw: f64, multiplier: f64) -> f64 {
    raw * multiplier
}

/// Raw BMS amperes (milliamperes) to amps. Sign convention: negative = discharging.
pub fn bms_amp_to_amps(raw: f64) -> f64 {
    raw * MILLI
}

pub fn bms_vol_to_volts(raw: f64) -> f64 {
    raw * MILLI
}

pub fn cell_vol_to_volts(raw: f64) -> f64 {
    raw * MILLI
}

/// Capacity in mAh at a given bus voltage, in Wh.
pub fn mah_at_volts_to_wh(cap_mah: f64, volts: f64) -> f64 {
    cap_mah * volts / 1000.0
}

pub fn watts_to_kw(w: f64) -> f64 {
    w / W_PER_KW
}

pub fn wh_to_kwh(wh: f64) -> f64 {
    wh / WH_PER_KWH
}

pub fn celsius_to_fahrenheit(c: f64) -> f64 {
    c * 1.8 + 32.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bms_scaling_matches_live_delta2max_values() {
        // Live capture: amp = -5183, vol = 52985, wattsOutSum = 218
        assert_eq!(bms_amp_to_amps(-5183.0), -5.183);
        assert_eq!(bms_vol_to_volts(52985.0), 52.985);
        // Watts keys on inv/mppt/pd.watts* are already whole watts.
        assert_eq!(scale(218.0, 1.0), 218.0);
    }

    #[test]
    fn mah_at_volts_gives_wh() {
        // 4000 mAh at 52.985 V ≈ 211.9 Wh
        assert!((mah_at_volts_to_wh(4000.0, 52.985) - 211.94).abs() < 0.01);
    }

    #[test]
    fn temperature_conversion() {
        assert_eq!(celsius_to_fahrenheit(0.0), 32.0);
        assert_eq!(celsius_to_fahrenheit(100.0), 212.0);
    }
}
