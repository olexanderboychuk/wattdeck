use chrono::{DateTime, Utc};

use crate::app::App;
use crate::devices::capabilities::DeviceCapabilities;
use crate::devices::registry::lookup;
use crate::telemetry::history::{Sample, mean_power, peak_powers};
use crate::telemetry::models::{Channel, DeviceSnapshot, Provenance, Totals};

/// One telemetry reading, taken once per render, that every panel draws from. The
/// point is that the overview, the power-flow panel, the distribution bars and the
/// history chart cannot disagree: they read the same `totals`, the same channel
/// list, the same sample window, and they are stamped with the same `at`.
pub struct Snapshot<'a> {
    pub device: Option<&'a DeviceSnapshot>,
    /// Capabilities of the selected model, so "not on this model" and "not
    /// reported this update" can be told apart.
    pub caps: DeviceCapabilities,
    pub totals: Totals,
    pub channels: Vec<Channel>,
    /// The in-memory sample window, oldest first.
    pub samples: Vec<Sample>,
    /// Peak over the sample window, not over the live values.
    pub peak_in: Option<f64>,
    pub peak_out: Option<f64>,
    pub mean_in: Option<f64>,
    pub mean_out: Option<f64>,
    /// The newest sample in the window, which can lag the live values: the chart's
    /// right edge is drawn from this, the "live" line from `totals`.
    pub latest: Option<Sample>,
    pub at: DateTime<Utc>,
}

impl Snapshot<'_> {
    pub fn sn(&self) -> &str {
        self.device.map(|d| d.sn.as_str()).unwrap_or("")
    }

    /// How a number was obtained, as the short marker the panels append to it.
    pub fn mark(p: Provenance) -> &'static str {
        match p {
            Provenance::Measured => "device",
            Provenance::Computed => "sum",
            Provenance::DeviceEstimate => "device est",
            Provenance::Unknown => "n/a",
        }
    }

    /// Where the newest sample came from: the chart's x extent in seconds.
    pub fn window_secs(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }
        let (first, last) = (self.samples[0].t, self.samples[self.samples.len() - 1].t);
        (last - first).num_seconds() as f64
    }
}

/// Build the render's snapshot. Cheap: references and a bounded ring copy.
pub fn capture(app: &App) -> Snapshot<'_> {
    let device = app.selected_device();
    let (caps, totals, channels) = match device {
        Some(d) => {
            let caps = lookup(d.product_type).caps;
            (caps, d.power.totals(caps), d.power.channels(caps))
        }
        None => (DeviceCapabilities::default(), Totals::default(), Vec::new()),
    };
    let samples = device
        .map(|d| app.history_for(&d.sn).ordered())
        .unwrap_or_default();
    let (peak_in, peak_out) = if samples.is_empty() {
        (None, None)
    } else {
        let (i, o) = peak_powers(&samples);
        (Some(i), Some(o))
    };
    Snapshot {
        device,
        caps,
        totals,
        channels,
        peak_in,
        peak_out,
        mean_in: (!samples.is_empty()).then(|| mean_power(&samples, |s| s.in_w)),
        mean_out: (!samples.is_empty()).then(|| mean_power(&samples, |s| s.out_w)),
        latest: samples.last().copied(),
        at: app.now,
        samples,
    }
}
