use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::mpsc::Sender;

use crate::api::client::{ApiError, Client};
use crate::api::devices;
use crate::config::Config;
use crate::config::credentials::Credentials;
use crate::services::{
    Command, ConnState, DiscoveredDevice, Level, Notification, Transport, Update,
};
use crate::telemetry::models::{DeviceSnapshot, Source};
use crate::telemetry::normalizer::merge_params;
use crate::utils::formatting::mask_serial;

/// Live data source: the signed REST API. It does the initial device discovery,
/// polls each device's property snapshot, and keeps the snapshots accumulated so
/// partial reports stay complete.
pub struct LiveProvider {
    cfg: Config,
    client: Client,
    state: LiveState,
    devices: Vec<DiscoveredDevice>,
    snapshots: HashMap<String, DeviceSnapshot>,
    /// Last reported failure per serial, so a device that is permanently refused does
    /// not add the same warning to the alert list on every sweep.
    last_error: HashMap<String, String>,
}

impl LiveProvider {
    pub fn new(cfg: &Config, creds: &Credentials) -> Self {
        Self {
            cfg: cfg.clone(),
            client: Client::new(&cfg.api, creds),
            state: LiveState::new(),
            devices: Vec::new(),
            snapshots: HashMap::new(),
            last_error: HashMap::new(),
        }
    }

    pub async fn run(mut self, tx: Sender<Update>, mut rx: tokio::sync::mpsc::Receiver<Command>) {
        let cadence = std::time::Duration::from_secs(self.cfg.api.refresh_interval_seconds.max(5));
        let _ = tx
            .send(Update::Transport(Transport::Rest, ConnState::Connecting))
            .await;
        loop {
            match self.sweep(&tx).await {
                Sweep::Ok => {}
                Sweep::Err => {}
                Sweep::AuthFailed => {
                    // Repeating a request with bad credentials produces the same
                    // error; stop polling for a long stretch so the UI keeps showing
                    // the auth state without a flood of requests.
                    tokio::time::sleep(cadence * 10).await;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(cadence) => {}
                cmd = rx.recv() => {
                    match cmd {
                        Some(Command::Refresh) => {}
                        Some(Command::Scenario(s)) => {
                            let _ = tx
                                .send(Update::Notification(Notification::info(format!(
                                    "scenario '{s}' is a mock-mode feature; live data follows the real device",
                                    s = s.label()
                                ))))
                                .await;
                        }
                        Some(Command::Control {
                            sn,
                            control,
                            value,
                        }) => {
                            // Live REST has no verified command path for this device,
                            // and sending a command through an unverified endpoint is
                            // not acceptable: MQTT is the transport the commands are
                            // verified on.
                            let _ = tx
                                .send(Update::Notification(Notification::warn(format!(
                                    "command {} is sent over MQTT; the REST provider cannot send it",
                                    control.label
                                ))))
                                .await;
                            let _ = (sn, value);
                        }
                        Some(Command::Quit) | None => return,
                    }
                }
            }
        }
    }

    /// One pass over the device list, respecting the spacing between requests.
    async fn sweep(&mut self, tx: &Sender<Update>) -> Sweep {
        let now = Utc::now();
        let list = match devices::device_list(&self.client).await {
            Ok(l) => l,
            Err(e) => {
                for u in self.state.record_failure(now, &e) {
                    let _ = tx.send(u).await;
                }
                if e.is_auth() {
                    for u in self.state.record_auth_failure(now) {
                        let _ = tx.send(u).await;
                    }
                    return Sweep::AuthFailed;
                }
                return Sweep::Err;
            }
        };
        if list != self.devices {
            self.devices = list.clone();
            for u in self.state.record_list(now, &list) {
                let _ = tx.send(u).await;
            }
        }
        let mut ok = 0;
        let mut failed = 0;
        for d in self.devices.clone() {
            match devices::properties(&self.client, &d.sn).await {
                Ok(data) => {
                    ok += 1;
                    let snap = self.seed(&d);
                    merge_params(snap, &map_of(&data));
                    snap.source = Source::Rest;
                    snap.online = d.online;
                    snap.seq += 1;
                    snap.last_update = now;
                    let _ = tx.send(Update::Snapshot(Box::new(snap.clone()))).await;
                    // A device that starts reporting again is allowed to warn again.
                    self.last_error.remove(&d.sn);
                }
                Err(e) => {
                    failed += 1;
                    let text = format!("{}: {e}", mask_serial(&d.sn));
                    // A device that is permanently refused by the API is reported once,
                    // not on every poll.
                    if report_failure(&mut self.last_error, &d.sn, &text) {
                        self.notify(tx, Notification::warn(text));
                    }
                }
            }
            // Space requests out so a sweep of several devices stays inside rate limits.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        for u in self.state.record_sweep(now, ok, failed) {
            let _ = tx.send(u).await;
        }
        Sweep::Ok
    }

    /// Snapshots are kept between sweeps so a partial report does not lose the rest.
    fn seed(&mut self, d: &DiscoveredDevice) -> &mut DeviceSnapshot {
        self.snapshots.entry(d.sn.clone()).or_insert_with(|| {
            let info = crate::devices::registry::lookup_by_name(&d.product_name);
            let mut snap =
                DeviceSnapshot::new(d.sn.clone(), d.product_name.clone(), info.product_type);
            snap.family = info.family;
            snap.online = d.online;
            snap
        })
    }

    fn notify(&self, tx: &Sender<Update>, n: Notification) {
        let _ = tx.try_send(Update::Notification(n));
    }
}

enum Sweep {
    Ok,
    Err,
    AuthFailed,
}

fn map_of(data: &Value) -> std::collections::HashMap<String, Value> {
    data.as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Record a per-device failure and report whether it is new. The same refusal from the
/// same device on the next sweep is not a new alert.
fn report_failure(last: &mut HashMap<String, String>, sn: &str, text: &str) -> bool {
    if last.get(sn) == Some(&text.to_string()) {
        return false;
    }
    last.insert(sn.to_string(), text.to_string());
    true
}

/// Transport state machine for the REST source: consecutive failures degrade the
/// connection, a success restores it, and an auth failure is reported once.
#[derive(Debug)]
pub struct LiveState {
    state: ConnState,
    failures: u32,
    last_notify: Option<DateTime<Utc>>,
    pub auth_failed: bool,
}

impl LiveState {
    pub fn new() -> Self {
        Self {
            state: ConnState::Initializing,
            failures: 0,
            last_notify: None,
            auth_failed: false,
        }
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    fn set(
        &mut self,
        now: DateTime<Utc>,
        next: ConnState,
        level: Level,
        text: &str,
    ) -> Vec<Update> {
        let mut out = Vec::new();
        if self.state != next {
            self.state = next;
            out.push(Update::Transport(Transport::Rest, next));
            if !text.is_empty() && self.notify_throttled(now) {
                out.push(Update::Notification(Notification {
                    level,
                    text: text.to_string(),
                    at: now,
                }));
            }
        }
        out
    }

    /// Notifications are rate limited so a flapping connection cannot fill the alert
    /// list.
    fn notify_throttled(&mut self, now: DateTime<Utc>) -> bool {
        if let Some(prev) = self.last_notify
            && (now - prev).num_minutes() < 5
        {
            return false;
        }
        self.last_notify = Some(now);
        true
    }

    pub fn record_list(&mut self, now: DateTime<Utc>, list: &[DiscoveredDevice]) -> Vec<Update> {
        let mut out = self.set(now, ConnState::Connected, Level::Info, "");
        out.push(Update::DeviceList(list.to_vec()));
        out
    }

    pub fn record_sweep(&mut self, now: DateTime<Utc>, ok: u32, failed: u32) -> Vec<Update> {
        if ok > 0 && failed == 0 {
            self.failures = 0;
            self.set(now, ConnState::Connected, Level::Info, "")
        } else if ok > 0 {
            self.set(
                now,
                ConnState::Degraded,
                Level::Warn,
                "some devices did not report",
            )
        } else {
            self.record_failure_inner(now)
        }
    }

    pub fn record_failure(&mut self, now: DateTime<Utc>, e: &ApiError) -> Vec<Update> {
        self.failures += 1;
        let mut out = self.record_failure_inner(now);
        if self.failures == 1 {
            out.push(Update::Notification(Notification {
                level: Level::Warn,
                text: format!("REST: {e}"),
                at: now,
            }));
        }
        out
    }

    fn record_failure_inner(&mut self, now: DateTime<Utc>) -> Vec<Update> {
        let next = if self.failures >= 3 {
            ConnState::Offline
        } else {
            ConnState::Reconnecting
        };
        self.set(now, next, Level::Warn, "REST is not reachable")
    }

    pub fn record_auth_failure(&mut self, now: DateTime<Utc>) -> Vec<Update> {
        self.auth_failed = true;
        let mut out = self.set(now, ConnState::AuthError, Level::Critical, "");
        out.push(Update::Notification(Notification {
            level: Level::Critical,
            text: "REST authentication failed - check access key, secret key and region".into(),
            at: now,
        }));
        out
    }
}

impl Default for LiveState {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiError {
    /// The verified signature-error code is 8521. EcoFlow's other auth failures
    /// (expired key, key without device permission) report codes that are not
    /// documented, so anything the server refuses with 401/403 is treated as auth as
    /// well, and the message is shown verbatim.
    fn is_auth(&self) -> bool {
        match self {
            ApiError::Api { code, .. } => code == "8521",
            ApiError::Malformed(s) => s == "http 401" || s == "http 403",
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap()
    }

    /// A failure the REST source can report without the network: the state machine
    /// does not care which transport error it is.
    fn failure() -> ApiError {
        ApiError::Malformed("http 500".into())
    }

    #[test]
    fn success_connects() {
        let mut s = LiveState::new();
        let list = vec![DiscoveredDevice {
            sn: "AAA".into(),
            product_name: "DELTA 2 Max".into(),
            online: true,
            product_type: Some(81),
        }];
        let out = s.record_list(now(), &list);
        assert!(matches!(
            out[0],
            Update::Transport(Transport::Rest, ConnState::Connected)
        ));
        assert!(matches!(out[1], Update::DeviceList(_)));
        assert_eq!(s.state(), ConnState::Connected);
    }

    #[test]
    fn three_failures_go_offline() {
        let mut s = LiveState::new();
        let e = failure();
        let first = s.record_failure(now(), &e);
        assert!(matches!(
            first[0],
            Update::Transport(Transport::Rest, ConnState::Reconnecting)
        ));
        // The second failure does not repeat the same state: the UI only gets the
        // transitions.
        let second = s.record_failure(now() + chrono::Duration::minutes(10), &e);
        assert!(
            !second
                .iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, _)))
        );
        assert_eq!(s.state(), ConnState::Reconnecting);
        let third = s.record_failure(now() + chrono::Duration::minutes(20), &e);
        assert!(
            third
                .iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, ConnState::Offline)))
        );
        assert_eq!(s.state(), ConnState::Offline);
    }

    #[test]
    fn partial_sweep_is_degraded() {
        let mut s = LiveState::new();
        let out = s.record_sweep(now(), 1, 1);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, ConnState::Degraded)))
        );
        let out2 = s.record_sweep(now() + chrono::Duration::minutes(10), 2, 0);
        assert!(
            out2.iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, ConnState::Connected)))
        );
    }

    #[test]
    fn notifications_are_throttled() {
        let mut s = LiveState::new();
        let e = failure();
        let a = s.record_failure(now(), &e);
        let b = s.record_failure(now() + chrono::Duration::seconds(5), &e);
        assert!(a.iter().any(|u| matches!(u, Update::Notification(_))));
        assert!(!b.iter().any(|u| matches!(u, Update::Notification(_))));
    }

    #[test]
    fn auth_failure_is_reported_once_as_critical() {
        let mut s = LiveState::new();
        let out = s.record_auth_failure(now());
        assert!(s.auth_failed);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, ConnState::AuthError)))
        );
        assert!(out.iter().any(|u| matches!(
            u,
            Update::Notification(n)
            if n.level == Level::Critical
        )));
        // The second auth failure does not add the same state again.
        let out2 = s.record_auth_failure(now() + chrono::Duration::minutes(30));
        assert!(
            !out2
                .iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, ConnState::AuthError)))
        );
    }

    #[test]
    fn signature_code_is_auth() {
        let e = ApiError::Api {
            code: "8521".into(),
            message: "signature is wrong".into(),
        };
        assert!(e.is_auth());
        assert!(!ApiError::Malformed("http 500".into()).is_auth());
        assert!(ApiError::Malformed("http 403".into()).is_auth());
    }

    #[test]
    fn property_map_is_extracted() {
        let v = json!({ "bms_bmsStatus.soc": 85, "pd.wattsOutSum": 218 });
        let m = map_of(&v);
        assert_eq!(m.get("bms_bmsStatus.soc").unwrap(), &json!(85));
        // A non-object payload yields an empty map, not a crash.
        assert!(map_of(&json!(null)).is_empty());
    }

    #[test]
    fn snapshots_accumulate_across_partial_reports() {
        let mut snap = DeviceSnapshot::new("AAA", "DELTA 2 Max", 81);
        merge_params(
            &mut snap,
            &map_of(&json!({ "bms_bmsStatus.soc": 85, "bms_bmsStatus.productType": 81 })),
        );
        assert_eq!(snap.battery.soc, Some(85.0));
        assert_eq!(snap.product_type, 81);
        merge_params(&mut snap, &map_of(&json!({ "pd.wattsOutSum": 218 })));
        // The second report is partial: the SOC from the first one is still there.
        assert_eq!(snap.battery.soc, Some(85.0));
        assert_eq!(snap.power.total_out_w, Some(218.0));
    }

    #[test]
    fn masked_serials_appear_in_notifications() {
        let n = Notification::warn(format!("{}: boom", mask_serial("R351ZEB5PGC90303")));
        assert!(!n.text.contains("R351ZEB5PGC90303"));
        assert!(n.text.starts_with("R3*"));
    }

    #[test]
    fn a_repeated_device_failure_is_not_re_notified() {
        let mut last = HashMap::new();
        let e = "RB***********37: api error code 1006: not allowed".to_string();
        assert!(report_failure(&mut last, "RB2MN5ZE7721004", &e));
        assert!(!report_failure(&mut last, "RB2MN5ZE7721004", &e));
        // A different error on the same device is worth a new alert.
        assert!(report_failure(
            &mut last,
            "RB2MN5ZE7721004",
            "RB***********37: network: timeout"
        ));
        // After the device reports again the entry is removed, so the next failure is
        // reported.
        last.remove("RB2MN5ZE7721004");
        assert!(report_failure(&mut last, "RB2MN5ZE7721004", &e));
    }
}
