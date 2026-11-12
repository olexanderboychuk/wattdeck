use chrono::{DateTime, Utc};

use crate::telemetry::history::{Sample, rolling_avg};
use crate::telemetry::models::{Battery, Provenance, Totals};

/// The largest value the EcoFlow runtime counters report: 5999 minutes = 99 h 59 m.
///
/// Verified from three angles: the published property tables give the range as
/// 0..143999 min, so 5999 is not the format ceiling; a live DELTA 2 Max reports
/// `bms_emsStatus.dsgRemainTime = 5999` while it is charging, where a discharge
/// countdown has no meaning; and users report the EcoFlow app showing "99:59" as the
/// time to full on a River 2 drawing 5-6 W. A counter pinned at its display ceiling
/// is what a station with no useful estimate reports — not a 99-hour runtime.
pub const SENTINEL_MIN: f64 = 5999.0;

/// Below this the station is not moving energy. An estimate built on a near-zero
/// power is a division by noise, so the app calls that state idle.
pub const IDLE_W: f64 = 5.0;

/// The window the app averages over when it computes a runtime itself. 15 minutes
/// of 5-second samples is 180 readings: enough to smooth a cloud passing over the
/// solar input.
pub const ESTIMATE_WINDOW_SECS: i64 = 900;

/// The sample count a rolling mean needs before the app calls it a mean.
pub const MIN_SAMPLES: usize = 3;

/// The station's operating state, from the measured flow first and the device's own
/// state code second. Charging, discharging, idle, full and unknown stay apart: the
/// same runtime number means something different in each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Idle,
    Full,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Charging => "charging",
            State::Discharging => "discharging",
            State::Idle => "idle",
            State::Full => "fully charged",
        }
    }
}

/// One runtime figure, with where it came from. `raw` is the device counter for the
/// active state, kept verbatim even when it is the 5999 sentinel, so the diagnostics
/// screen can show what the device said.
#[derive(Debug, Clone, Copy)]
pub struct Estimate {
    pub state: State,
    pub minutes: Option<f64>,
    pub provenance: Provenance,
    pub raw: Option<f64>,
    pub raw_key: &'static str,
}

impl Estimate {
    /// The plain-language line the dashboard shows. Nothing is implied: a computed
    /// figure says it is calculated, a device figure says the device reported it,
    /// and an unavailable one says that.
    pub fn label(&self) -> String {
        match (self.state, self.minutes, self.provenance) {
            (State::Full, _, _) => "fully charged".to_string(),
            (State::Idle, _, _) => "idle, no load".to_string(),
            (State::Unknown, _, _) => "state unknown".to_string(),
            (State::Charging, Some(m), Provenance::DeviceEstimate) => {
                format!("charging, full in {}", human(m))
            }
            (State::Charging, Some(m), _) => format!("charging, full in {} (calc)", human(m)),
            (State::Discharging, Some(m), Provenance::DeviceEstimate) => {
                format!("discharging, {} left", human(m))
            }
            (State::Discharging, Some(m), _) => format!("discharging, {} left (calc)", human(m)),
            (State::Charging, None, _) => "charging, time to full not reported".to_string(),
            (State::Discharging, None, _) => "discharging, time left not reported".to_string(),
        }
    }
}

/// A counter pinned at 5999 = 99 h 59 m. The device uses it when it has no estimate
/// for the current state, so it is never read as a runtime.
pub fn is_sentinel(min: f64) -> bool {
    min >= SENTINEL_MIN
}

fn human(min: f64) -> String {
    let total = min.round() as i64;
    if total <= 0 {
        "under a minute".to_string()
    } else if total >= 120 {
        format!("{} h {} m", total / 60, total % 60)
    } else {
        format!("{total} m")
    }
}

/// The state, from what the meter measures. `pd.chgDsgState` is the code the device
/// shows on its own screen; it is used only when nothing is moving, and never
/// overrides a measured flow.
pub fn state(b: &Battery, totals: &Totals, max_charge_soc: f64) -> State {
    let Some(soc) = b.soc else {
        return State::Unknown;
    };
    if soc >= max_charge_soc {
        return State::Full;
    }
    if let Some(p) = b.bus_power_w() {
        if p > 0.0 {
            return State::Charging;
        }
        if p < 0.0 {
            return State::Discharging;
        }
    }
    if totals.out_w.is_some_and(|w| w > IDLE_W) {
        return State::Discharging;
    }
    if totals.in_w.is_some_and(|w| w > IDLE_W) {
        return State::Charging;
    }
    State::Idle
}

/// The runtime for the state the station is in.
///
/// `window` is the sample history, oldest first; the average is taken over the last
/// `ESTIMATE_WINDOW_SECS` of it. Device counters are preferred, and a counter that
/// belongs to the other state is never used: a charging station's
/// `dsgRemainTime` (5999 on a live unit) is not a discharge runtime.
pub fn estimate(
    b: &Battery,
    totals: &Totals,
    window: &[Sample],
    max_charge_soc: f64,
    now: DateTime<Utc>,
) -> Estimate {
    let s = state(b, totals, max_charge_soc);
    let (raw, raw_key) = match s {
        State::Charging => device_counter(b, Counter::Charge),
        State::Discharging => device_counter(b, Counter::Discharge),
        State::Full => (b.chg_remain_min, "bms_emsStatus.chgRemainTime"),
        State::Idle | State::Unknown => first_counter(b),
    };
    let computed = match s {
        State::Charging => charge_estimate(b, window, now),
        State::Discharging => discharge_estimate(b, window, totals, now),
        State::Full | State::Idle | State::Unknown => None,
    };
    // A counter only answers for the state it belongs to, so a station at rest or at
    // its charge limit never reports a device runtime.
    let device_minutes = matches!(s, State::Charging | State::Discharging)
        .then(|| usable(raw))
        .flatten();
    if let Some(m) = device_minutes {
        return Estimate {
            state: s,
            minutes: Some(m),
            provenance: Provenance::DeviceEstimate,
            raw,
            raw_key,
        };
    }
    Estimate {
        state: s,
        minutes: computed,
        provenance: if computed.is_some() {
            Provenance::Computed
        } else {
            Provenance::Unknown
        },
        raw,
        raw_key,
    }
}

/// The first runtime counter the device reported, in the order the keys are read.
/// For a station at rest there is no runtime, and the raw value is kept for the
/// diagnostics screen.
fn first_counter(b: &Battery) -> (Option<f64>, &'static str) {
    [
        (b.chg_remain_min, "bms_emsStatus.chgRemainTime"),
        (b.dsg_remain_min, "bms_emsStatus.dsgRemainTime"),
        (b.bms_remain_min, "bms_bmsStatus.remainTime"),
        (b.lcd_remain_min, "pd.remainTime"),
    ]
    .into_iter()
    .find(|(v, _)| v.is_some())
    .unwrap_or((None, "pd.remainTime"))
}

/// Which device counter belongs to which state. On a live DELTA 2 Max the charging
/// counter reads 7 minutes while `dsgRemainTime` sits at 5999, and
/// `bms_bmsStatus.remainTime` is signed: positive is charging time, negative is
/// discharging time.
enum Counter {
    Charge,
    Discharge,
}

fn device_counter(b: &Battery, which: Counter) -> (Option<f64>, &'static str) {
    match which {
        Counter::Charge => (
            b.chg_remain_min
                .or_else(|| b.bms_remain_min.filter(|v| *v > 0.0))
                .or_else(|| b.lcd_remain_min.filter(|v| *v > 0.0)),
            "bms_emsStatus.chgRemainTime",
        ),
        Counter::Discharge => (
            b.dsg_remain_min
                .or_else(|| b.bms_remain_min.filter(|v| *v < 0.0))
                .or_else(|| b.lcd_remain_min.filter(|v| *v > 0.0)),
            "bms_emsStatus.dsgRemainTime",
        ),
    }
}

/// A counter the app can use: present, not the 5999 sentinel, and not zero, which is
/// what a device leaves a counter at when the state is not active.
fn usable(min: Option<f64>) -> Option<f64> {
    min.filter(|m| *m > 0.0 && !is_sentinel(*m))
}

/// Time to full from the energy headroom and the measured charging power.
fn charge_estimate(b: &Battery, window: &[Sample], now: DateTime<Utc>) -> Option<f64> {
    let (capacity, remaining) = (b.capacity_wh?, b.remaining_wh?);
    let headroom = capacity - remaining;
    if headroom <= 0.0 {
        return Some(0.0);
    }
    let power = mean_charge_w(b, window, now)?;
    Some(headroom / power * 60.0)
}

/// Time to empty from the energy left and the measured draw.
fn discharge_estimate(
    b: &Battery,
    window: &[Sample],
    totals: &Totals,
    now: DateTime<Utc>,
) -> Option<f64> {
    let remaining = b.remaining_wh?;
    if remaining <= 0.0 {
        return Some(0.0);
    }
    let power = mean_draw_w(window, totals, now)?;
    Some(remaining / power * 60.0)
}

/// Average charging power over the estimate window, from the battery ammeter. Falls
/// back to the instantaneous measurement when history is thin, so the first minute
/// after startup still produces an estimate.
fn mean_charge_w(b: &Battery, window: &[Sample], now: DateTime<Utc>) -> Option<f64> {
    let (avg, n) = rolling_avg(window, now, ESTIMATE_WINDOW_SECS, |s| s.bat_w.max(0.0));
    if n >= MIN_SAMPLES && avg > IDLE_W {
        return Some(avg);
    }
    b.bus_power_w().filter(|p| *p > IDLE_W)
}

/// Average draw, from the battery ammeter, falling back to the reported output
/// power. `pd.wattsOutSum` includes the inverter's own draw, so it is a fair measure
/// of what the station is spending.
fn mean_draw_w(window: &[Sample], totals: &Totals, now: DateTime<Utc>) -> Option<f64> {
    let (avg, n) = rolling_avg(window, now, ESTIMATE_WINDOW_SECS, |s| {
        s.bat_w.min(0.0).abs()
    });
    if n >= MIN_SAMPLES && avg > IDLE_W {
        return Some(avg);
    }
    totals.out_w.filter(|w| *w > IDLE_W)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc::now() + Duration::seconds(secs)
    }

    fn sample(secs: i64, bat_w: f64, out_w: f64) -> Sample {
        Sample {
            t: at(secs),
            soc: 50.0,
            in_w: 0.0,
            out_w,
            solar_w: 0.0,
            temp_c: 25.0,
            bat_w,
        }
    }

    /// Oldest first, as the app's ring buffer stores it.
    fn window(secs: i64, bat_w: f64, out_w: f64) -> Vec<Sample> {
        let mut v: Vec<Sample> = (0..6)
            .map(|i| sample(secs - i * 10, bat_w, out_w))
            .collect();
        v.sort_by_key(|s| s.t);
        v
    }

    fn charging() -> Battery {
        Battery {
            soc: Some(49.0),
            remaining_wh: Some(1000.0),
            capacity_wh: Some(2048.0),
            amp_a: Some(5.0),
            volt_v: Some(50.0),
            chg_remain_min: Some(7.0),
            dsg_remain_min: Some(5999.0),
            lcd_remain_min: Some(411.0),
            pd_chg_dsg_raw: Some(1.0),
            ..Default::default()
        }
    }

    fn discharging() -> Battery {
        Battery {
            soc: Some(50.0),
            remaining_wh: Some(1000.0),
            capacity_wh: Some(2048.0),
            amp_a: Some(-2.0),
            volt_v: Some(50.0),
            dsg_remain_min: Some(411.0),
            pd_chg_dsg_raw: Some(0.0),
            ..Default::default()
        }
    }

    fn draw() -> Totals {
        Totals {
            out_w: Some(100.0),
            out_src: Provenance::Measured,
            ..Default::default()
        }
    }

    #[test]
    fn charging_prefers_the_device_counter() {
        let b = charging();
        let e = estimate(
            &b,
            &Totals::default(),
            &window(0, 250.0, 0.0),
            100.0,
            Utc::now(),
        );
        assert_eq!(e.state, State::Charging);
        assert_eq!(e.minutes, Some(7.0), "the device's own time to full");
        assert_eq!(e.provenance, Provenance::DeviceEstimate);
        assert_eq!(e.raw, Some(7.0), "the counter for the active state is kept");
        assert_eq!(
            e.label(),
            "charging, full in 7 m",
            "plain words, no jargon: {}",
            e.label()
        );
    }

    #[test]
    fn the_5999_counter_is_never_read_as_a_runtime() {
        let mut b = charging();
        b.chg_remain_min = Some(5999.0);
        let e = estimate(
            &b,
            &Totals::default(),
            &window(0, 250.0, 0.0),
            100.0,
            Utc::now(),
        );
        assert_eq!(e.state, State::Charging);
        // 1048 Wh of headroom at the averaged 250 W = 4.192 h
        let m = e.minutes.expect("a computed estimate");
        assert!((m - 251.52).abs() < 0.5);
        assert_eq!(
            e.provenance,
            Provenance::Computed,
            "the app says where it came from"
        );
        assert_eq!(
            e.raw,
            Some(5999.0),
            "the raw value is preserved, not discarded"
        );
    }

    #[test]
    fn a_discharging_station_uses_the_discharge_counter() {
        let b = discharging();
        let e = estimate(&b, &draw(), &window(0, -100.0, 100.0), 100.0, Utc::now());
        assert_eq!(e.state, State::Discharging);
        assert_eq!(e.minutes, Some(411.0));
        assert_eq!(e.provenance, Provenance::DeviceEstimate);
    }

    #[test]
    fn no_device_counter_means_the_app_computes() {
        let mut b = discharging();
        b.dsg_remain_min = None;
        let e = estimate(&b, &draw(), &window(0, -100.0, 100.0), 100.0, Utc::now());
        // 1000 Wh at the averaged 100 W = 6 h
        assert!((e.minutes.unwrap() - 600.0).abs() < 1.0);
        assert_eq!(e.provenance, Provenance::Computed);
    }

    #[test]
    fn idle_reports_no_runtime() {
        let mut b = discharging();
        b.amp_a = Some(0.0);
        b.dsg_remain_min = Some(5999.0);
        let e = estimate(
            &b,
            &Totals::default(),
            &window(0, 0.0, 0.0),
            100.0,
            Utc::now(),
        );
        assert_eq!(e.state, State::Idle);
        assert_eq!(e.minutes, None, "a near-zero draw is not a runtime");
        assert_eq!(e.label(), "idle, no load");
        assert_eq!(e.raw, Some(5999.0), "the raw counter is still kept");
    }

    #[test]
    fn full_reports_no_runtime() {
        let mut b = charging();
        b.soc = Some(100.0);
        let e = estimate(
            &b,
            &Totals::default(),
            &window(0, 250.0, 0.0),
            100.0,
            Utc::now(),
        );
        assert_eq!(e.state, State::Full);
        assert_eq!(e.minutes, None);
        assert_eq!(e.label(), "fully charged");
    }

    #[test]
    fn a_station_at_its_charge_ceiling_is_full() {
        let mut b = charging();
        b.soc = Some(91.0);
        let e = estimate(
            &b,
            &Totals::default(),
            &window(0, 250.0, 0.0),
            90.0,
            Utc::now(),
        );
        assert_eq!(
            e.state,
            State::Full,
            "the user's charge limit is the ceiling"
        );
        assert_eq!(
            e.minutes, None,
            "a station at its limit is not charging to 90%"
        );
    }

    #[test]
    fn missing_telemetry_is_unknown() {
        let b = Battery::default();
        let e = estimate(&b, &Totals::default(), &[], 100.0, Utc::now());
        assert_eq!(e.state, State::Unknown);
        assert_eq!(e.minutes, None);
        assert_eq!(e.label(), "state unknown");
    }

    #[test]
    fn samples_outside_the_window_are_not_averaged() {
        // History older than 15 minutes is not averaged; the estimate uses the live
        // measurement, so a station that stopped reporting does not get a runtime
        // built on data it has not sent for an hour.
        let mut b = discharging();
        b.dsg_remain_min = None;
        let totals = Totals {
            out_w: Some(120.0),
            out_src: Provenance::Measured,
            ..Default::default()
        };
        // Samples 20 minutes old: outside the 15-minute averaging window.
        let old = window(-1200, -400.0, 400.0);
        let e = estimate(&b, &totals, &old, 100.0, Utc::now());
        // 1000 Wh at the live 120 W, not the stale 400 W
        assert!((e.minutes.unwrap() - 500.0).abs() < 1.0);
    }

    #[test]
    fn the_sentinel_is_the_ceiling_the_device_reports() {
        assert!(is_sentinel(5999.0));
        assert!(!is_sentinel(5998.0));
        assert!(
            is_sentinel(143999.0),
            "above the ceiling is not a runtime either"
        );
    }
}
