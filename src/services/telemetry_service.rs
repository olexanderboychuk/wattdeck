use std::collections::HashMap;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

use crate::services::{Command, ConnState, DiscoveredDevice, Notification, Transport, Update};
use crate::telemetry::models::{DeviceSnapshot, Source};
use crate::telemetry::normalizer;

/// Deterministic scenarios so the interface can be exercised in known states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scenario {
    #[default]
    SolarCharging,
    AcCharging,
    DischargingHighLoad,
    FullyCharged,
    CriticalBattery,
    Offline,
}

impl Scenario {
    pub const ALL: [Scenario; 6] = [
        Scenario::SolarCharging,
        Scenario::AcCharging,
        Scenario::DischargingHighLoad,
        Scenario::FullyCharged,
        Scenario::CriticalBattery,
        Scenario::Offline,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Scenario::SolarCharging => "solar charging",
            Scenario::AcCharging => "AC charging",
            Scenario::DischargingHighLoad => "high load discharging",
            Scenario::FullyCharged => "fully charged",
            Scenario::CriticalBattery => "critical battery",
            Scenario::Offline => "device offline",
        }
    }

    pub fn next(self) -> Scenario {
        let i = Scenario::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Scenario::ALL[(i + 1) % Scenario::ALL.len()]
    }

    pub fn from_name(name: &str) -> Option<Scenario> {
        Scenario::ALL.iter().find(|s| s.label() == name).copied()
    }
}

/// A deterministic generator: no floating randomness, so a run of a scenario
/// reproduces the same sequence for tests.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(636413622380714347)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }

    /// Value in `[-amplitude, +amplitude]`.
    pub fn jitter(&mut self, amplitude: f64) -> f64 {
        (self.next_f64() * 2.0 - 1.0) * amplitude
    }
}

pub struct MockDevice {
    pub sn: String,
    pub product_name: String,
    pub product_type: u16,
    pub capacity_wh: f64,
    pub design_cap_mah: f64,
    pub soc: f64,
    pub solar1_w: f64,
    pub solar2_w: f64,
    pub ac_in_w: f64,
    pub ac_out_w: f64,
    pub usb_w: f64,
    pub car_w: f64,
    pub temp_c: f64,
    pub soh: f64,
    pub cycles: f64,
    pub online: bool,
    pub phase: f64,
    /// The configuration the device reports on every push. A real device keeps
    /// these settings between snapshots, and a command the device accepts changes
    /// them, so the mock stores them the same way: `charge_limit` 100,
    /// `discharge_limit` 5, AC output on, DC output on, AC charge 400 W,
    /// standby 30 min, beep off, screen off after 300 s — the values the live
    /// DELTA 2 Max reported on 2026-11-10.
    pub max_chg_soc: i64,
    pub min_dsg_soc: i64,
    pub ac_on: bool,
    pub dc_on: bool,
    pub slow_chg_w: i64,
    pub standby_min: i64,
    pub beep: bool,
    pub lcd_sec: i64,
    /// Local tick counter, used for the incremental publishing cadence.
    pub tick: u64,
}

impl MockDevice {
    pub fn delta2_max() -> Self {
        Self {
            sn: "MOCKR351ZEB5PGC9030".to_string(),
            product_name: "DELTA 2 Max".to_string(),
            product_type: 81,
            capacity_wh: 2048.0,
            design_cap_mah: 4096000.0 / 52.985,
            soc: 82.0,
            solar1_w: 0.0,
            solar2_w: 0.0,
            ac_in_w: 0.0,
            ac_out_w: 0.0,
            usb_w: 0.0,
            car_w: 0.0,
            temp_c: 29.0,
            soh: 99.0,
            cycles: 64.0,
            online: true,
            max_chg_soc: 100,
            min_dsg_soc: 5,
            ac_on: true,
            dc_on: true,
            slow_chg_w: 400,
            standby_min: 30,
            beep: false,
            lcd_sec: 300,
            // Start near the peak of the solar curve so the solar scenario opens
            // in its characteristic state, not at sunrise.
            phase: 0.5,
            tick: 0,
        }
    }

    pub fn river() -> Self {
        Self {
            sn: "MOCKRBEBZ511111111".to_string(),
            product_name: "RIVER 2 Pro".to_string(),
            product_type: 72,
            capacity_wh: 256.0,
            design_cap_mah: 4096000.0 / 24.0,
            soc: 45.0,
            solar1_w: 0.0,
            solar2_w: 0.0,
            ac_in_w: 0.0,
            ac_out_w: 0.0,
            usb_w: 0.0,
            car_w: 0.0,
            temp_c: 26.0,
            soh: 97.0,
            cycles: 128.0,
            online: false,
            max_chg_soc: 100,
            min_dsg_soc: 0,
            ac_on: true,
            dc_on: true,
            slow_chg_w: 200,
            standby_min: 30,
            beep: true,
            lcd_sec: 300,
            phase: 0.3,
            tick: 0,
        }
    }

    fn in_w(&self) -> f64 {
        self.solar1_w + self.solar2_w + self.ac_in_w
    }

    fn out_w(&self) -> f64 {
        self.ac_out_w + self.usb_w + self.car_w
    }

    /// Produce the partial blocks a real device would publish on this tick.
    /// Keys use the dotted names the real DELTA 2 Max uses.
    fn publish(
        &mut self,
        scenario: Scenario,
        dt: f64,
        rng: &mut Lcg,
    ) -> Vec<HashMap<String, Value>> {
        // An offline device publishes nothing, exactly as a real device that has no
        // MQTT connection. The UI marks it offline from the device list.
        if !self.online {
            return Vec::new();
        }
        self.phase = (self.phase + dt / 600.0) % 1.0;
        match scenario {
            Scenario::SolarCharging => {
                let curve = (self.phase * std::f64::consts::PI).sin().max(0.0);
                self.solar1_w = curve * 520.0 + rng.jitter(25.0);
                self.solar2_w = curve * 310.0 + rng.jitter(20.0);
                self.ac_in_w = 0.0;
                self.ac_out_w = 60.0 + rng.jitter(15.0);
                self.usb_w = 18.0 + rng.jitter(6.0);
                self.car_w = 0.0;
                self.temp_c = 27.0 + self.in_w() / 120.0 + rng.jitter(1.0);
            }
            Scenario::AcCharging => {
                self.solar1_w = 0.0;
                self.solar2_w = 0.0;
                self.ac_in_w = 650.0 + rng.jitter(30.0);
                self.ac_out_w = 90.0 + rng.jitter(20.0);
                self.usb_w = 30.0 + rng.jitter(8.0);
                self.car_w = 0.0;
                self.temp_c = 31.0 + self.ac_in_w / 240.0 + rng.jitter(1.0);
            }
            Scenario::DischargingHighLoad => {
                self.solar1_w = 0.0;
                self.solar2_w = 0.0;
                self.ac_in_w = 0.0;
                self.ac_out_w = 1250.0 + rng.jitter(120.0);
                self.usb_w = 60.0 + rng.jitter(10.0);
                self.car_w = 120.0 + rng.jitter(20.0);
                self.temp_c = 36.0 + self.ac_out_w / 400.0 + rng.jitter(1.5);
            }
            Scenario::FullyCharged => {
                self.soc = 100.0;
                self.solar1_w = 0.0;
                self.solar2_w = 0.0;
                self.ac_in_w = 0.0;
                self.ac_out_w = 220.0 + rng.jitter(30.0);
                self.usb_w = 20.0;
                self.car_w = 0.0;
                self.temp_c = 28.0 + rng.jitter(1.0);
            }
            Scenario::CriticalBattery => {
                self.soc = self.soc.clamp(4.0, 12.0);
                self.solar1_w = 0.0;
                self.solar2_w = 0.0;
                self.ac_in_w = 0.0;
                self.ac_out_w = 300.0 + rng.jitter(60.0);
                self.usb_w = 10.0;
                self.car_w = 0.0;
                self.temp_c = 41.0 + rng.jitter(2.0);
            }
            Scenario::Offline => {
                self.ac_out_w = 150.0 + rng.jitter(40.0);
                self.usb_w = 15.0;
                self.temp_c = 30.0 + rng.jitter(1.0);
            }
        }
        // The AC output switch: an inverter that is switched off delivers nothing,
        // which is what a real device reports after the `acOutCfg` command.
        if !self.ac_on {
            self.ac_out_w = 0.0;
        }
        // Energy bookkeeping: charge is power integrated over time (W·s → Wh).
        let net_w = self.in_w() - self.out_w();
        self.soc += net_w * dt / (self.capacity_wh * 3600.0) * 100.0;
        self.soc = self.soc.clamp(0.0, 100.0);
        // The charge limit is a ceiling, so the mock never reports more than the
        // value the device is configured to stop at.
        self.soc = self.soc.min(self.max_chg_soc as f64);

        let mut blocks = Vec::new();
        // pd block: totals, ports.
        let mut pd: HashMap<String, Value> = HashMap::new();
        pd.insert("pd.wattsInSum".into(), Value::from(self.in_w()));
        pd.insert("pd.wattsOutSum".into(), Value::from(self.out_w()));
        pd.insert("pd.usb1Watts".into(), Value::from(self.usb_w));
        pd.insert("pd.carWatts".into(), Value::from(self.car_w * 10.0));
        pd.insert("pd.soc".into(), Value::from(self.soc.round() as i64));
        pd.insert(
            "pd.acEnabled".into(),
            Value::from(if self.ac_out_w > 0.0 { 1 } else { 0 }),
        );
        pd.insert("pd.dcOutState".into(), Value::from(self.dc_on as i64));
        pd.insert("pd.standbyMin".into(), Value::from(self.standby_min));
        pd.insert("pd.beepMode".into(), Value::from(self.beep as i64));
        pd.insert("pd.lcdOffSec".into(), Value::from(self.lcd_sec));
        blocks.push(pd);

        // inv + mppt block
        let mut inv: HashMap<String, Value> = HashMap::new();
        inv.insert("inv.inputWatts".into(), Value::from(self.ac_in_w));
        inv.insert("inv.outputWatts".into(), Value::from(self.ac_out_w));
        inv.insert("inv.acInVol".into(), Value::from(230000.0));
        inv.insert("inv.invOutVol".into(), Value::from(230100.0));
        inv.insert("inv.outTemp".into(), Value::from(self.temp_c));
        inv.insert("inv.cfgAcEnabled".into(), Value::from(self.ac_on as i64));
        inv.insert("inv.SlowChgWatts".into(), Value::from(self.slow_chg_w));
        inv.insert("inv.FastChgWatts".into(), Value::from(2400));
        inv.insert("mppt.inWatts".into(), Value::from(self.solar1_w));
        inv.insert("mppt.pv2InWatts".into(), Value::from(self.solar2_w));
        inv.insert("mppt.inVol".into(), Value::from(34000.0));
        inv.insert(
            "mppt.inAmp".into(),
            Value::from(self.solar1_w / 34.0 * 1000.0),
        );
        inv.insert("mppt.mpptTemp".into(), Value::from(self.temp_c + 3.0));
        blocks.push(inv);

        // bms block every third tick, like a real device's incremental cadence
        if self.tick.is_multiple_of(3) {
            let mut bms: HashMap<String, Value> = HashMap::new();
            bms.insert(
                "bms_bmsStatus.soc".into(),
                Value::from(self.soc.round() as i64),
            );
            bms.insert("bms_bmsStatus.f32ShowSoc".into(), Value::from(self.soc));
            bms.insert(
                "bms_bmsStatus.amp".into(),
                Value::from(net_w / 52.985 * 1000.0),
            );
            bms.insert("bms_bmsStatus.vol".into(), Value::from(52985.0));
            bms.insert(
                "bms_bmsStatus.remainCap".into(),
                Value::from(self.soc / 100.0 * self.design_cap_mah),
            );
            bms.insert(
                "bms_bmsStatus.designCap".into(),
                Value::from(self.design_cap_mah),
            );
            bms.insert("bms_bmsStatus.temp".into(), Value::from(self.temp_c));
            bms.insert(
                "bms_bmsStatus.maxCellTemp".into(),
                Value::from(self.temp_c + 1.0),
            );
            bms.insert("bms_bmsStatus.soh".into(), Value::from(self.soh));
            bms.insert(
                "bms_emsStatus.maxChargeSoc".into(),
                Value::from(self.max_chg_soc),
            );
            bms.insert(
                "bms_emsStatus.minDsgSoc".into(),
                Value::from(self.min_dsg_soc),
            );
            bms.insert(
                "bms_bmsStatus.cycles".into(),
                Value::from(self.cycles as i64),
            );
            bms.insert(
                "bms_bmsStatus.cellVol".into(),
                Value::from(vec![3300, 3310, 3295, 3305, 3312, 3300, 3298, 3301]),
            );
            bms.insert(
                "bms_bmsStatus.productType".into(),
                Value::from(self.product_type as i64),
            );
            blocks.push(bms);
        }
        self.tick += 1;
        blocks
    }
}

pub struct MockSim {
    pub devices: Vec<MockDevice>,
    pub scenario: Scenario,
    rng: Lcg,
    tick: u64,
    mqtt_down_for: u64,
    pub snapshots: Vec<DeviceSnapshot>,
}

impl MockSim {
    pub fn new(scenario: Scenario, seed: u64) -> Self {
        let devices = vec![MockDevice::delta2_max(), MockDevice::river()];
        let snapshots: Vec<DeviceSnapshot> = devices
            .iter()
            .map(|d| DeviceSnapshot::new(d.sn.clone(), d.product_name.clone(), d.product_type))
            .collect();
        Self {
            devices,
            scenario,
            rng: Lcg::new(seed),
            tick: 0,
            mqtt_down_for: 0,
            snapshots,
        }
    }

    /// A command the user confirmed. The mock behaves the way the device the
    /// command is aimed at does: the value is checked against the range the control
    /// declares, an accepted value is stored as the device's configuration and
    /// written to the property key the device reports, so the next push shows it,
    /// and the answer is the shape a `set_reply` gives (`0` accepted, `-3` a value
    /// the device would not take, `-1` a serial this session does not have).
    pub fn accept(
        &mut self,
        sn: &str,
        control: &crate::controls::Control,
        value: i64,
    ) -> Vec<Update> {
        let (min, max) = match control.kind {
            crate::controls::ValueKind::Toggle => (0, 1),
            crate::controls::ValueKind::Number { min, max, .. } => (min, max),
        };
        if !control.accepts(value) {
            return vec![Update::ControlReply {
                sn: sn.to_string(),
                code: "-3".to_string(),
                message: format!(
                    "{value} is outside the range {min}..{max} that {} accepts",
                    control.label
                ),
            }];
        }
        let Some(i) = self.snapshots.iter().position(|s| s.sn == sn) else {
            return vec![Update::ControlReply {
                sn: sn.to_string(),
                code: "-1".to_string(),
                message: "this session has no device with that serial".to_string(),
            }];
        };
        // A device keeps its configuration between pushes, so the accepted value
        // is stored on the mock device and reported from the next snapshot on.
        let d = &mut self.devices[i];
        match control.id {
            "charge_limit" => d.max_chg_soc = value,
            "discharge_limit" => d.min_dsg_soc = value,
            "ac_output" => d.ac_on = value != 0,
            "dc_output" => d.dc_on = value != 0,
            "ac_charge_watts" => d.slow_chg_w = value,
            "standby_minutes" => d.standby_min = value,
            "beep" => d.beep = value != 0,
            "lcd_timeout" => d.lcd_sec = value,
            _ => {}
        }
        let s = &mut self.snapshots[i];
        s.raw
            .insert(control.read_key.to_string(), Value::from(value));
        normalizer::renorm(s);
        s.seq += 1;
        s.last_update = Utc::now();
        let snapshot = Update::Snapshot(Box::new(s.clone()));
        vec![
            snapshot,
            Update::ControlReply {
                sn: sn.to_string(),
                code: "0".to_string(),
                message: "mock accepted".to_string(),
            },
        ]
    }

    /// One simulation step: returns updates to send. The merged snapshots go through
    /// the same normalizer the MQTT path uses.
    pub fn step(&mut self, dt: f64) -> Vec<Update> {
        self.tick += 1;
        let mut out = Vec::new();

        // Simulate MQTT dropping for a few ticks every 30 ticks.
        if self.tick % 30 == 7 {
            self.mqtt_down_for = 6;
            out.push(Update::Notification(Notification::warn(
                "MQTT connection dropped (mock)",
            )));
        }
        let mqtt_state = if self.mqtt_down_for > 0 {
            self.mqtt_down_for -= 1;
            ConnState::Reconnecting
        } else {
            ConnState::Connected
        };
        out.push(Update::Transport(Transport::Rest, ConnState::Connected));
        out.push(Update::Transport(Transport::Mqtt, mqtt_state));

        for (i, dev) in self.devices.iter_mut().enumerate() {
            dev.online = dev.product_type != 72; // RIVER stays offline in the mock
            let blocks = dev.publish(self.scenario, dt, &mut self.rng);
            if dev.online {
                let snap = &mut self.snapshots[i];
                for block in &blocks {
                    normalizer::merge_params(snap, block);
                }
                snap.online = true;
                snap.last_update = Utc::now();
                snap.source = Source::Mock;
                snap.seq += 1;
                out.push(Update::Snapshot(Box::new(snap.clone())));
            } else {
                let snap = &mut self.snapshots[i];
                snap.online = false;
                snap.source = Source::Mock;
                out.push(Update::Snapshot(Box::new(snap.clone())));
            }
        }

        out
    }

    pub fn device_list(&self) -> Vec<DiscoveredDevice> {
        self.devices
            .iter()
            .map(|d| DiscoveredDevice {
                sn: d.sn.clone(),
                product_name: d.product_name.clone(),
                online: d.online,
                product_type: Some(d.product_type),
            })
            .collect()
    }

    /// Set the connection state the mock reports for the given transport.
    pub fn transport_state(&self, t: Transport) -> ConnState {
        match t {
            Transport::Rest => ConnState::Connected,
            Transport::Mqtt => {
                if self.mqtt_down_for > 0 {
                    ConnState::Reconnecting
                } else {
                    ConnState::Connected
                }
            }
        }
    }
}

/// Run the mock provider: emits device list, then a stream of partial updates.
/// It also listens for UI commands (scenario change, refresh), so keyboard scenario
/// switching works in mock mode.
pub async fn run_mock(
    scenario: Scenario,
    tx: Sender<Update>,
    mut rx: tokio::sync::mpsc::Receiver<Command>,
    tick_secs: f64,
) {
    let mut sim = MockSim::new(scenario, 0x5EED);
    let _ = tx.send(Update::DeviceList(sim.device_list())).await;
    let _ = tx
        .send(Update::Notification(Notification::info(
            "mock provider running; no live device is contacted",
        )))
        .await;
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs_f64(tick_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                for u in sim.step(tick_secs) {
                    if tx.send(u).await.is_err() {
                        return;
                    }
                }
            }
            cmd = rx.recv() => {
                match cmd {
                    Some(Command::Scenario(s)) => {
                        sim.scenario = s;
                        // Restart the simulation so the scenario starts from a known state.
                        sim.devices = vec![MockDevice::delta2_max(), MockDevice::river()];
                        sim.snapshots = sim
                            .devices
                            .iter()
                            .map(|d| {
                                DeviceSnapshot::new(
                                    d.sn.clone(),
                                    d.product_name.clone(),
                                    d.product_type,
                                )
                            })
                            .collect();
                        sim.tick = 0;
                        sim.mqtt_down_for = 0;
                    }
                    Some(Command::Refresh) => {
                        let _ = tx.send(Update::Notification(Notification::info("refresh: mock state re-emitted"))).await;
                    }
                    Some(Command::Control { sn, control, value }) => {
                        for u in sim.accept(&sn, control, value) {
                            let _ = tx.send(u).await;
                        }
                    }
                    Some(Command::Quit) | None => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::devices::registry::Family;

    #[test]
    fn scenarios_are_distinct_and_cycle() {
        let mut s = Scenario::SolarCharging;
        for _ in 0..6 {
            s = s.next();
        }
        assert_eq!(s, Scenario::SolarCharging, "cycle returns to start");
        assert_eq!(Scenario::ALL.len(), 6);
    }

    #[test]
    fn names_round_trip() {
        for s in Scenario::ALL {
            assert_eq!(Scenario::from_name(s.label()), Some(s));
        }
        assert_eq!(Scenario::from_name("nope"), None);
    }

    #[test]
    fn lcg_is_deterministic() {
        let mut a = Lcg::new(12345);
        let mut b = Lcg::new(12345);
        for _ in 0..100 {
            assert_eq!(a.next_f64(), b.next_f64());
        }
    }

    #[test]
    fn lcg_jitter_is_bounded() {
        let mut r = Lcg::new(999);
        for _ in 0..500 {
            let v = r.jitter(10.0);
            assert!(v.abs() <= 10.0, "jitter {v} out of range");
        }
    }

    #[test]
    fn mock_energy_moves_the_battery() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 42);
        let before = sim.devices[0].soc;
        for _ in 0..20 {
            sim.step(10.0);
        }
        // Solar input exceeds output, so charge must rise.
        assert!(sim.devices[0].soc > before);
    }

    #[test]
    fn high_load_drains_the_battery() {
        let mut sim = MockSim::new(Scenario::DischargingHighLoad, 42);
        let before = sim.devices[0].soc;
        for _ in 0..20 {
            sim.step(10.0);
        }
        assert!(sim.devices[0].soc < before);
    }

    #[test]
    fn soc_never_leaves_0_100() {
        let mut sim = MockSim::new(Scenario::DischargingHighLoad, 42);
        for _ in 0..500 {
            sim.step(10.0);
            for d in &sim.devices {
                assert!((0.0..=100.0).contains(&d.soc));
            }
        }
    }

    #[test]
    fn mock_updates_go_through_the_normalizer() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 42);
        let updates = sim.step(10.0);
        let snap = updates.iter().find_map(|u| match u {
            Update::Snapshot(s) if s.product_type == 81 => Some(s),
            _ => None,
        });
        let snap = snap.expect("delta 2 max snapshot");
        assert!(snap.power.solar_in_w.unwrap() > 0.0);
        assert_eq!(snap.source, Source::Mock);
        assert!(snap.battery.amp_a.is_some());
        assert_eq!(snap.family, crate::devices::registry::Family::Delta2);
    }

    #[test]
    fn offline_device_is_reported_offline() {
        let mut sim = MockSim::new(Scenario::Offline, 42);
        let updates = sim.step(10.0);
        let river = updates.iter().find_map(|u| match u {
            Update::Snapshot(s) if s.product_type == 72 => Some(s),
            _ => None,
        });
        assert!(!river.unwrap().online);
    }

    #[test]
    fn mqtt_simulated_drop_then_recovery() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 42);
        for _ in 0..7 {
            sim.step(10.0);
        }
        assert_eq!(
            sim.transport_state(Transport::Mqtt),
            ConnState::Reconnecting
        );
        for _ in 0..10 {
            sim.step(10.0);
        }
        assert_eq!(sim.transport_state(Transport::Mqtt), ConnState::Connected);
    }

    #[test]
    fn device_list_has_both_devices() {
        let sim = MockSim::new(Scenario::SolarCharging, 42);
        let list = sim.device_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].product_name, "DELTA 2 Max");
        assert_eq!(list[1].product_name, "RIVER 2 Pro");
    }

    #[test]
    fn partial_blocks_are_incremental_not_full_snapshots() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 42);
        let blocks = sim.devices[0].publish(Scenario::SolarCharging, 10.0, &mut Lcg::new(7));
        // The bms block does not contain every key of the 240-key live dump.
        let bms = blocks.iter().find(|b| b.contains_key("bms_bmsStatus.soc"));
        assert!(bms.is_some());
        assert!(!bms.unwrap().contains_key("pd.wattsOutSum"));
    }

    /// The control the mock is expected to answer, by id.
    fn control(id: &str) -> &'static crate::controls::Control {
        crate::controls::for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == id)
            .expect("control")
    }

    #[test]
    fn an_accepted_command_becomes_what_the_device_reports() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 1);
        // The ems block is published every third tick, so the reported limit starts
        // at the value the live device reports.
        for _ in 0..3 {
            sim.step(10.0);
        }
        assert_eq!(
            sim.snapshots[0].raw["bms_emsStatus.maxChargeSoc"],
            Value::from(100)
        );
        let out = sim.accept("MOCKR351ZEB5PGC9030", control("charge_limit"), 80);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::ControlReply { code, .. } if code == "0")),
            "the mock accepts a value inside the range"
        );
        assert_eq!(
            sim.snapshots[0].raw["bms_emsStatus.maxChargeSoc"],
            Value::from(80),
            "the reported key changes at once"
        );
        assert_eq!(
            sim.devices[0].max_chg_soc, 80,
            "the device keeps the setting"
        );
        // The next pushes keep reporting the new configuration, as a real device
        // does: a setting is not lost between snapshots.
        for _ in 0..6 {
            sim.step(10.0);
        }
        assert_eq!(
            sim.snapshots[0].raw["bms_emsStatus.maxChargeSoc"],
            Value::from(80)
        );
        // The charge limit is a ceiling, so the battery is never shown above it.
        assert!(
            sim.devices[0].soc <= 80.0,
            "the mock stops charging at the limit it was told to"
        );
    }

    #[test]
    fn a_value_outside_the_range_is_refused_and_changes_nothing() {
        let mut sim = MockSim::new(Scenario::SolarCharging, 1);
        sim.step(10.0);
        let out = sim.accept("MOCKR351ZEB5PGC9030", control("ac_charge_watts"), 3000);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::ControlReply { code, .. } if code == "-3")),
            "out of range is refused, not silently accepted"
        );
        assert_eq!(
            sim.snapshots[0].raw["inv.SlowChgWatts"],
            Value::from(400),
            "a refused command changes nothing"
        );
        assert_eq!(sim.devices[0].slow_chg_w, 400);
        assert!(
            out.iter().all(|u| !matches!(u, Update::Snapshot(_))),
            "a refusal does not emit a snapshot"
        );
    }

    #[test]
    fn switching_the_ac_output_off_stops_the_ac_ports() {
        let mut sim = MockSim::new(Scenario::DischargingHighLoad, 1);
        sim.step(10.0);
        assert!(
            sim.snapshots[0].power.ac_out_w > Some(0.0),
            "the high-load scenario delivers AC power"
        );
        sim.accept("MOCKR351ZEB5PGC9030", control("ac_output"), 0);
        sim.step(10.0);
        assert_eq!(
            sim.snapshots[0].settings.ac_enabled,
            Some(false),
            "the switch reports off"
        );
        assert_eq!(
            sim.snapshots[0].power.ac_out_w,
            Some(0.0),
            "an inverter that is switched off delivers nothing"
        );
    }
}
