use chrono::{DateTime, Duration, Utc};

use crate::telemetry::history::Sample;

/// A time bucket of aggregated energy. The analytics screen uses these, so a
/// day/week figure is computed the same way as the live view: the same trapezoid
/// rule, the same gap rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub energy_in_wh: f64,
    pub energy_out_wh: f64,
    pub solar_wh: f64,
    pub peak_in_w: f64,
    pub peak_out_w: f64,
    pub samples: usize,
}

/// Bucket samples into fixed-width intervals. Energy is the trapezoid integral of
/// power over each interval; intervals longer than `max_gap_secs` (a device that
/// stopped reporting) are skipped rather than counted as energy.
pub fn bucketize(samples: &[Sample], bucket_secs: i64, max_gap_secs: i64) -> Vec<Bucket> {
    if samples.len() < 2 || bucket_secs <= 0 {
        return Vec::new();
    }
    let mut buckets: Vec<Bucket> = Vec::new();
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dt = (b.t - a.t).num_seconds() as f64;
        if dt <= 0.0 || dt > max_gap_secs as f64 {
            continue;
        }
        let bucket_index = b.t.timestamp().div_euclid(bucket_secs);
        let start = DateTime::from_timestamp(bucket_index * bucket_secs, 0).unwrap_or(b.t);
        let end = start + Duration::seconds(bucket_secs);
        match buckets.last_mut() {
            Some(last) if last.start == start => {
                last.end = last.end.max(end);
                last.energy_in_wh += (a.in_w + b.in_w) / 2.0 * dt / 3600.0;
                last.energy_out_wh += (a.out_w + b.out_w) / 2.0 * dt / 3600.0;
                last.solar_wh += (a.solar_w + b.solar_w) / 2.0 * dt / 3600.0;
                last.peak_in_w = last.peak_in_w.max(a.in_w.max(b.in_w));
                last.peak_out_w = last.peak_out_w.max(a.out_w.max(b.out_w));
                last.samples += 1;
            }
            _ => {
                buckets.push(Bucket {
                    start,
                    end,
                    energy_in_wh: (a.in_w + b.in_w) / 2.0 * dt / 3600.0,
                    energy_out_wh: (a.out_w + b.out_w) / 2.0 * dt / 3600.0,
                    solar_wh: (a.solar_w + b.solar_w) / 2.0 * dt / 3600.0,
                    peak_in_w: a.in_w.max(b.in_w),
                    peak_out_w: a.out_w.max(b.out_w),
                    samples: 1,
                });
            }
        }
    }
    buckets
}

/// The bucket width that suits an analytics range: short ranges get fine buckets,
/// long ranges get day or week buckets, so the table never runs past the panel.
pub fn bucket_width_for(range_secs: i64) -> i64 {
    if range_secs <= 900 {
        60
    } else if range_secs <= 3600 {
        300
    } else if range_secs <= 21600 {
        3600
    } else if range_secs <= 604800 {
        86400
    } else {
        604800
    }
}

/// The bucket width in words, for the panel title.
pub fn width_name(width_secs: i64) -> &'static str {
    match width_secs {
        60 => "per minute",
        300 => "per 5 min",
        3600 => "hourly",
        86400 => "daily",
        _ => "weekly",
    }
}

/// A bucket's label, at the width the bucket was made with.
pub fn bucket_label(start: &DateTime<Utc>, width_secs: i64) -> String {
    if width_secs >= 86400 {
        start.format("%m-%d").to_string()
    } else {
        start.format("%H:%M").to_string()
    }
}

/// Energy price from a user-configurable tariff (currency per kWh).
pub fn cost_wh(energy_wh: f64, tariff_per_kwh: f64) -> f64 {
    energy_wh / 1000.0 * tariff_per_kwh
}

pub fn merge_buckets(buckets: &[Bucket]) -> Bucket {
    let mut out = Bucket {
        start: buckets.first().map(|b| b.start).unwrap_or(Utc::now()),
        end: buckets.last().map(|b| b.end).unwrap_or(Utc::now()),
        energy_in_wh: 0.0,
        energy_out_wh: 0.0,
        solar_wh: 0.0,
        peak_in_w: 0.0,
        peak_out_w: 0.0,
        samples: 0,
    };
    for b in buckets {
        out.energy_in_wh += b.energy_in_wh;
        out.energy_out_wh += b.energy_out_wh;
        out.solar_wh += b.solar_wh;
        out.peak_in_w = out.peak_in_w.max(b.peak_in_w);
        out.peak_out_w = out.peak_out_w.max(b.peak_out_w);
        out.samples += b.samples;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_at(t: DateTime<Utc>, in_w: f64, out_w: f64, solar: f64) -> Sample {
        Sample {
            t,
            soc: 50.0,
            in_w,
            out_w,
            solar_w: solar,
            temp_c: 25.0,
        }
    }

    /// Samples 10 minutes apart starting at 10:00, so the intervals stay inside the
    /// 900 s gap limit and bucket boundaries are exact.
    fn at(min: i64, in_w: f64, out_w: f64, solar: f64) -> Sample {
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        sample_at(base + Duration::minutes(min), in_w, out_w, solar)
    }

    #[test]
    fn hourly_bucket_of_a_steady_load() {
        let s = [
            at(0, 100.0, 0.0, 0.0),
            at(10, 100.0, 0.0, 0.0),
            at(25, 100.0, 0.0, 0.0),
        ];
        let b = bucketize(&s, 3600, 900);
        assert_eq!(b.len(), 1);
        // 100 W over 600 s + 900 s = 41.67 Wh
        assert!((b[0].energy_in_wh - 41.6667).abs() < 0.01);
        assert_eq!(b[0].samples, 2);
        assert_eq!(b[0].peak_in_w, 100.0);
        assert_eq!(
            b[0].start,
            Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn gaps_are_not_counted() {
        // A two-hour gap is a device that stopped reporting, not 2000 Wh of charging.
        let s = [at(0, 1000.0, 0.0, 0.0), at(120, 1000.0, 0.0, 0.0)];
        let b = bucketize(&s, 3600, 900);
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn crossing_a_bucket_boundary_makes_two_buckets() {
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 10, 40, 0).unwrap();
        let s = [
            sample_at(base, 200.0, 100.0, 50.0),
            sample_at(base + Duration::minutes(10), 200.0, 100.0, 50.0),
            sample_at(base + Duration::minutes(25), 200.0, 100.0, 50.0),
        ];
        let b = bucketize(&s, 3600, 900);
        assert_eq!(b.len(), 2);
        assert_eq!(
            b[1].start,
            Utc.with_ymd_and_hms(2026, 1, 1, 11, 0, 0).unwrap()
        );
        let total = merge_buckets(&b);
        assert_eq!(total.samples, 2);
        // 200 W * (600 s + 900 s) = 83.33 Wh
        assert!((total.energy_in_wh - 83.3333).abs() < 0.01);
        assert!((total.energy_out_wh - 41.6667).abs() < 0.01);
        assert_eq!(total.peak_in_w, 200.0);
    }

    #[test]
    fn cost_uses_the_tariff() {
        assert_eq!(cost_wh(1000.0, 0.25), 0.25);
        assert_eq!(cost_wh(2500.0, 0.20), 0.5);
        assert_eq!(cost_wh(0.0, 1.0), 0.0);
    }

    #[test]
    fn bucket_widths_match_the_range() {
        assert_eq!(bucket_width_for(900), 60, "15 min: one-minute buckets");
        assert_eq!(bucket_width_for(3600), 300, "1 hour: five-minute buckets");
        assert_eq!(bucket_width_for(21600), 3600, "6 hours: hourly");
        assert_eq!(bucket_width_for(86400), 86400, "24 hours: daily");
        assert_eq!(bucket_width_for(604800), 86400, "7 days: daily");
        assert_eq!(bucket_width_for(2592000), 604800, "30 days: weekly");
    }

    #[test]
    fn width_names_read_as_english() {
        assert_eq!(width_name(60), "per minute");
        assert_eq!(width_name(300), "per 5 min");
        assert_eq!(width_name(3600), "hourly");
        assert_eq!(width_name(86400), "daily");
        assert_eq!(width_name(604800), "weekly");
    }

    #[test]
    fn labels_use_the_width_they_were_bucketed_with() {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        assert_eq!(bucket_label(&start, 300), "10:00");
        assert_eq!(bucket_label(&start, 86400), "01-01");
    }

    #[test]
    fn fewer_than_two_samples_yields_nothing() {
        assert!(bucketize(&[], 3600, 900).is_empty());
        assert!(bucketize(&[at(0, 100.0, 0.0, 0.0)], 3600, 900).is_empty());
    }
}
