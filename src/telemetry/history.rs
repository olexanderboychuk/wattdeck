use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};

/// A stored sample. Powers are instantaneous, energy is integrated over the interval.
/// `bat_w` is the battery-side power the BMS ammeter measured, signed: positive is
/// charging. It is what the runtime estimate averages over, because the battery side
/// sees the charging losses the input side does not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: DateTime<Utc>,
    pub soc: f64,
    pub in_w: f64,
    pub out_w: f64,
    pub solar_w: f64,
    pub temp_c: f64,
    pub bat_w: f64,
}

impl Sample {
    /// A sample built from what the device reported. Returns `None` when the device
    /// has reported no measurement at all: a station that has sent nothing is not a
    /// station measuring zero, and storing zeros would put numbers on the chart the
    /// station never produced.
    pub fn from_device(
        t: DateTime<Utc>,
        d: &crate::telemetry::models::DeviceSnapshot,
    ) -> Option<Self> {
        let measured = d.battery.soc.is_some()
            || d.power.total_in_w.is_some()
            || d.power.total_out_w.is_some();
        if !measured {
            return None;
        }
        Some(Self {
            t,
            soc: d.battery.soc.unwrap_or(0.0),
            in_w: d.power.total_in_w.unwrap_or(0.0),
            out_w: d.power.total_out_w.unwrap_or(0.0),
            solar_w: d.power.solar_in_w.unwrap_or(0.0),
            temp_c: d.battery.temp_c.unwrap_or(0.0),
            bat_w: d.battery.bus_power_w().unwrap_or(0.0),
        })
    }

    /// Whether two samples are the same measurement, ignoring the instant they were
    /// taken. An unchanged report is not stored again; the last identical sample is
    /// repeated on the heartbeat, so history says the station is still answering.
    pub fn same_measurement(&self, other: &Sample) -> bool {
        (
            self.soc,
            self.in_w,
            self.out_w,
            self.solar_w,
            self.temp_c,
            self.bat_w,
        ) == (
            other.soc,
            other.in_w,
            other.out_w,
            other.solar_w,
            other.temp_c,
            other.bat_w,
        )
    }
}

/// Fixed-capacity ring of recent samples per device, for sparklines.
/// Bounded: the app never grows memory with the number of MQTT messages.
#[derive(Debug, Clone)]
pub struct History {
    capacity: usize,
    samples: Vec<Sample>,
    head: usize,
    len: usize,
}

impl History {
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity: if capacity == 0 { 1 } else { capacity },
            samples: Vec::new(),
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, s: Sample) {
        if self.samples.len() < self.capacity {
            self.samples.push(s);
            self.len = self.samples.len();
        } else {
            self.samples[self.head] = s;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Oldest to newest.
    pub fn ordered(&self) -> Vec<Sample> {
        if self.len < self.capacity {
            return self.samples[..self.len].to_vec();
        }
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.samples[(self.head + i) % self.capacity]);
        }
        out
    }

    pub fn latest(&self) -> Option<Sample> {
        if self.len == 0 {
            return None;
        }
        Some(self.samples[(self.head + self.len - 1) % self.capacity])
    }
}

/// Trapezoidal integration of power over time, in watt-hours. Watts summed
/// directly is meaningless; energy is the integral of power.
pub fn integrate_energy(samples: &[Sample], field: fn(&Sample) -> f64) -> f64 {
    let mut wh = 0.0;
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dt = (b.t - a.t).num_seconds() as f64;
        if dt <= 0.0 {
            continue;
        }
        // Guard against a device being offline for hours: a single huge interval
        // is a gap, not a measurement, so skip intervals longer than 15 minutes.
        if dt > 900.0 {
            continue;
        }
        wh += (field(&a) + field(&b)) / 2.0 * dt;
    }
    // The accumulation is in W·s; energy is reported in Wh.
    wh / 3600.0
}

/// Bucket samples into calendar days (UTC), returning day -> (in Wh, out Wh, solar Wh).
pub fn daily_totals(samples: &[Sample]) -> BTreeMap<String, (f64, f64, f64)> {
    let mut days: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dt = (b.t - a.t).num_seconds() as f64;
        if dt <= 0.0 || dt > 900.0 {
            continue;
        }
        let day = b.t.date_naive().to_string();
        let entry = days.entry(day).or_insert((0.0, 0.0, 0.0));
        entry.0 += (a.in_w + b.in_w) / 2.0 * dt;
        entry.1 += (a.out_w + b.out_w) / 2.0 * dt;
        entry.2 += (a.solar_w + b.solar_w) / 2.0 * dt;
    }
    // Convert J-scaled values (W·s) to Wh.
    for v in days.values_mut() {
        v.0 /= 3600.0;
        v.1 /= 3600.0;
        v.2 /= 3600.0;
    }
    days
}

pub fn peak_powers(samples: &[Sample]) -> (f64, f64) {
    let mut peak_in: f64 = 0.0;
    let mut peak_out: f64 = 0.0;
    for s in samples {
        peak_in = peak_in.max(s.in_w);
        peak_out = peak_out.max(s.out_w);
    }
    (peak_in, peak_out)
}

pub fn mean_power(samples: &[Sample], field: fn(&Sample) -> f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(field).sum::<f64>() / samples.len() as f64
}

/// Mean of `field` over the samples inside the last `secs` of `now`, with the sample
/// count, so a caller can tell a mean from a single reading. Samples outside the
/// window are not averaged: history the station has stopped sending is not evidence
/// of what it is doing now.
pub fn rolling_avg(
    samples: &[Sample],
    now: DateTime<Utc>,
    secs: i64,
    field: fn(&Sample) -> f64,
) -> (f64, usize) {
    let cutoff = now - Duration::seconds(secs);
    let inside: Vec<f64> = samples
        .iter()
        .filter(|s| s.t >= cutoff && s.t <= now)
        .map(field)
        .collect();
    if inside.is_empty() {
        return (0.0, 0);
    }
    (
        inside.iter().sum::<f64>() / inside.len() as f64,
        inside.len(),
    )
}

pub fn sample_span(samples: &[Sample]) -> Duration {
    if samples.len() < 2 {
        return Duration::zero();
    }
    samples.last().unwrap().t - samples.first().unwrap().t
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Fixed base so spans and integrations are exact: a test that depends on
    /// `Utc::now()` cannot assert an exact number of seconds.
    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
    }

    fn sample(min: i64, in_w: f64, out_w: f64) -> Sample {
        Sample {
            t: base() - Duration::minutes(min),
            // sample times are relative to the base, oldest first when min decreases
            soc: 50.0,
            in_w,
            out_w,
            solar_w: 0.0,
            temp_c: 25.0,
            bat_w: 0.0,
        }
    }

    #[test]
    fn ring_keeps_capacity_and_order() {
        let mut h = History::new(3);
        for i in 0..5u32 {
            h.push(sample(5 - i as i64, i as f64, 0.0));
        }
        assert_eq!(h.len(), 3);
        let ordered = h.ordered();
        assert_eq!(ordered.len(), 3);
        assert!(ordered[0].in_w < ordered[2].in_w);
        assert_eq!(h.latest().unwrap().in_w, 4.0);
    }

    #[test]
    fn energy_is_the_integral_of_power() {
        // 100 W for 10 minutes = 100 * 600 J = 60000 J = 16.67 Wh
        let s = [sample(10, 100.0, 0.0), sample(0, 100.0, 0.0)];
        let wh = integrate_energy(&s, |x| x.in_w);
        assert!((wh - 16.666).abs() < 0.01);
    }

    #[test]
    fn trapezoid_of_ramp() {
        let s = [
            sample(2, 0.0, 0.0),
            sample(1, 200.0, 0.0),
            sample(0, 0.0, 0.0),
        ];
        // (0+200)/2*60 + (200+0)/2*60 = 6000 + 6000 = 12000 J = 3.33 Wh
        let wh = integrate_energy(&s, |x| x.in_w);
        assert!((wh - 3.333).abs() < 0.01);
    }

    #[test]
    fn offline_gap_is_not_integrated() {
        let s = [sample(120, 1000.0, 0.0), sample(0, 1000.0, 0.0)];
        assert_eq!(integrate_energy(&s, |x| x.in_w), 0.0);
    }

    #[test]
    fn daily_totals_split_across_days() {
        let d1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let d2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let mk = |t: DateTime<Utc>| Sample {
            t,
            soc: 10.0,
            in_w: 100.0,
            out_w: 0.0,
            solar_w: 0.0,
            temp_c: 20.0,
            bat_w: -100.0,
        };
        // Two pairs, 10 minutes apart inside each day: 100 W * 600 s = 16.67 Wh.
        let totals = daily_totals(&[
            mk(d1),
            mk(d1 + Duration::minutes(10)),
            mk(d2),
            mk(d2 + Duration::minutes(10)),
        ]);
        assert_eq!(totals.len(), 2);
        let first = totals.values().next().unwrap();
        assert!((first.0 - 16.6667).abs() < 0.01);
    }

    #[test]
    fn peaks_and_means() {
        let s = [
            sample(2, 100.0, 10.0),
            sample(1, 300.0, 50.0),
            sample(0, 200.0, 20.0),
        ];
        assert_eq!(peak_powers(&s), (300.0, 50.0));
        assert!((mean_power(&s, |x| x.in_w) - 200.0).abs() < 0.01);
        assert_eq!(sample_span(&s), Duration::minutes(2));
    }

    #[test]
    fn rolling_average_stays_inside_its_window() {
        let now = base();
        let s = [
            sample(30, 0.0, 0.0), // 30 minutes old: outside a 15-minute window
            sample(9, 100.0, 0.0),
            sample(4, 200.0, 0.0),
            sample(1, 300.0, 0.0),
        ];
        let (avg, n) = rolling_avg(&s, now, 900, |x| x.in_w);
        assert_eq!(n, 3, "the 30-minute-old sample is not averaged");
        assert!((avg - 200.0).abs() < 0.01);
        let (avg, n) = rolling_avg(&s, now, 60, |x| x.in_w);
        assert_eq!((avg, n), (300.0, 1), "one sample inside one minute");
    }

    #[test]
    fn empty_history_is_safe() {
        let h = History::new(10);
        assert!(h.is_empty());
        assert_eq!(h.latest(), None);
        assert_eq!(integrate_energy(&[], |x| x.in_w), 0.0);
        assert_eq!(peak_powers(&[]), (0.0, 0.0));
        assert_eq!(mean_power(&[], |x| x.in_w), 0.0);
        assert_eq!(rolling_avg(&[], base(), 900, |x| x.in_w), (0.0, 0));
    }
}
