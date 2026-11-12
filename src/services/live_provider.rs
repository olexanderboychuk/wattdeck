use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::mpsc::Sender;

use crate::api::client::{ApiError, Client};
use crate::api::devices;
use crate::config::Config;
use crate::config::credentials::Credentials;
use crate::services::{
    Command, ConnState, DiscoveredDevice, Level, Notification, Transport, TransportInfo, Update,
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
            .send(Update::Transport(
                Transport::Rest,
                self.state.info(ConnState::Connecting),
            ))
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
    ///
    /// Two kinds of failure are counted separately. A request the server refused for
    /// one device (a business code, not an HTTP failure) is that device's problem:
    /// the transport answered, so the connection stays online and the device is
    /// reported as refused. A request that did not get a good answer - network, 4xx,
    /// 5xx - is a transport failure.
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
        let mut refused = 0;
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
                    snap.last_rest = Some(now);
                    let _ = tx.send(Update::Snapshot(Box::new(snap.clone()))).await;
                    // A device that starts reporting again is allowed to warn again.
                    self.last_error.remove(&d.sn);
                }
                Err(e) => {
                    let (code, text) = e.signature();
                    if is_device_refusal(&e) {
                        refused += 1;
                        // The refusal names the device and says what monitoring
                        // effect it has; the same refusal is not repeated every poll.
                        let notice = format!(
                            "{}: the cloud refused this device (code {code}: {text}). \
                             Data for this device is not available.",
                            mask_serial(&d.sn)
                        );
                        if report_failure(&mut self.last_error, &d.sn, &notice) {
                            let _ = tx
                                .send(Update::DeviceRefusal {
                                    sn: d.sn.clone(),
                                    code: code.clone(),
                                    message: text,
                                })
                                .await;
                            self.notify(tx, Notification::warn(notice));
                        }
                    } else {
                        failed += 1;
                        let text = format!("{}: {e}", mask_serial(&d.sn));
                        for u in self.state.record_failure(now, &e) {
                            let _ = tx.send(u).await;
                        }
                        if report_failure(&mut self.last_error, &d.sn, &text) {
                            self.notify(tx, Notification::warn(text));
                        }
                    }
                }
            }
            // Space requests out so a sweep of several devices stays inside rate limits.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        for u in self.state.record_sweep(now, ok, refused, failed) {
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

/// The server answered the request and refused the device: a business code, not a
/// transport failure. Only these codes say "this device", so they never degrade the
/// connection of the devices that are reporting.
fn is_device_refusal(e: &ApiError) -> bool {
    matches!(e, ApiError::Api { .. } | ApiError::NoData { .. })
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
/// connection, a success restores it, and an auth failure is reported once. The
/// counters the diagnostics screen reads live here.
#[derive(Debug)]
pub struct LiveState {
    state: ConnState,
    failures: u32,
    last_ok: Option<DateTime<Utc>>,
    last_fail: Option<DateTime<Utc>>,
    status: Option<u16>,
    error: Option<String>,
    reconnects: u32,
    last_notify: Option<DateTime<Utc>>,
    pub auth_failed: bool,
}

impl LiveState {
    pub fn new() -> Self {
        Self {
            state: ConnState::Initializing,
            failures: 0,
            last_ok: None,
            last_fail: None,
            status: None,
            error: None,
            reconnects: 0,
            last_notify: None,
            auth_failed: false,
        }
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    /// The diagnostics snapshot to send with a transition.
    fn info(&self, state: ConnState) -> TransportInfo {
        TransportInfo {
            state,
            last_ok: self.last_ok,
            last_fail: self.last_fail,
            status: self.status,
            error: self.error.clone(),
            fails: self.failures,
            reconnects: self.reconnects,
        }
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
            // A transport that comes back after being down is one reconnect.
            if next == ConnState::Connected && self.state != ConnState::Initializing {
                self.reconnects += 1;
            }
            self.state = next;
            if next == ConnState::Connected {
                self.last_ok = Some(now);
            }
            out.push(Update::Transport(Transport::Rest, self.info(next)));
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

    /// One sweep: `ok` requests answered, `refused` devices the server refused,
    /// `failed` requests that did not get a good answer.
    ///
    /// Refusals do not count against the connection. A device the account cannot
    /// read is not a network problem, and a REST sweep that answered for every other
    /// device keeps MQTT-fed telemetry valid.
    pub fn record_sweep(
        &mut self,
        now: DateTime<Utc>,
        ok: u32,
        refused: u32,
        failed: u32,
    ) -> Vec<Update> {
        if failed == 0 {
            self.failures = 0;
            // Refused devices are not a connection problem: a sweep that only refused
            // devices keeps the transport online.
            tracing::debug!("sweep: {ok} answered, {refused} refused, {failed} failed");
            self.set(now, ConnState::Connected, Level::Info, "")
        } else if ok > 0 {
            self.set(
                now,
                ConnState::Degraded,
                Level::Warn,
                "the cloud is answering some requests",
            )
        } else {
            self.record_failure_inner(now)
        }
    }

    pub fn record_failure(&mut self, now: DateTime<Utc>, e: &ApiError) -> Vec<Update> {
        self.failures += 1;
        self.last_fail = Some(now);
        self.status = e.http_status();
        self.error = Some(e.signature().1);
        let mut out = self.record_failure_inner(now);
        if self.failures == 1 {
            out.push(Update::Notification(Notification {
                level: Level::Warn,
                text: format!("cloud connection: {e}"),
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
        self.set(now, next, Level::Warn, "the cloud is not reachable")
    }

    pub fn record_auth_failure(&mut self, now: DateTime<Utc>) -> Vec<Update> {
        self.auth_failed = true;
        let mut out = self.set(now, ConnState::AuthError, Level::Critical, "");
        out.push(Update::Notification(Notification {
            level: Level::Critical,
            text: "Cloud sign-in failed. Check your API credentials.".into(),
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

    /// The API code, and the HTTP status when the server answered with one. Used for
    /// diagnostics; never contains a key, a nonce, or a signature.
    fn signature(&self) -> (String, String) {
        match self {
            ApiError::Api { code, message } => (code.clone(), message.clone()),
            ApiError::NoData { code } => (code.clone(), "no data".to_string()),
            ApiError::Malformed(s) => (String::new(), s.clone()),
            ApiError::Network(e) => (String::new(), format!("network: {e}")),
        }
    }

    /// The HTTP status the failure carries, when there is one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            ApiError::Malformed(s) => s.strip_prefix("http ").and_then(|p| p.parse::<u16>().ok()),
            ApiError::Network(e) => e.status().map(|s| s.as_u16()),
            _ => None,
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
    fn success_connects_and_records_the_success() {
        let mut s = LiveState::new();
        let list = vec![DiscoveredDevice {
            sn: "AAA".into(),
            product_name: "DELTA 2 Max".into(),
            online: true,
            product_type: Some(81),
        }];
        let out = s.record_list(now(), &list);
        let state = transport_state(&out[0]);
        assert_eq!(state, ConnState::Connected);
        assert!(matches!(out[1], Update::DeviceList(_)));
        assert_eq!(s.state(), ConnState::Connected);
        let info = s.info(ConnState::Connected);
        assert_eq!(
            info.last_ok,
            Some(now()),
            "the last good answer is recorded"
        );
        assert_eq!(info.fails, 0);
    }

    #[test]
    fn three_failures_go_offline() {
        let mut s = LiveState::new();
        let e = failure();
        let first = s.record_failure(now(), &e);
        assert_eq!(transport_state(&first[0]), ConnState::Reconnecting);
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
                .any(|u| matches!(u, Update::Transport(Transport::Rest, _)))
        );
        assert_eq!(s.state(), ConnState::Offline);
        assert_eq!(
            s.status,
            Some(500),
            "the status code is kept for diagnostics"
        );
        assert_eq!(s.failures, 3);
    }

    #[test]
    fn a_refused_device_does_not_degrade_the_connection() {
        // The live case: the cloud answers for the DELTA 2 Max and refuses the
        // RIVER mini. The transport answered, so it stays online.
        let mut s = LiveState::new();
        let out = s.record_sweep(now(), 1, 1, 0);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, _))
                    && transport_state(u) == ConnState::Connected),
            "one refusal is not a connection problem"
        );
        assert_eq!(s.state(), ConnState::Connected);
        assert_eq!(s.failures, 0);
    }

    #[test]
    fn a_failed_request_degrades_the_connection() {
        let mut s = LiveState::new();
        let out = s.record_sweep(now(), 1, 0, 1);
        assert!(
            out.iter()
                .any(|u| matches!(u, Update::Transport(Transport::Rest, _))
                    && transport_state(u) == ConnState::Degraded)
        );
        let out2 = s.record_sweep(now() + chrono::Duration::minutes(10), 2, 0, 0);
        assert!(
            out2.iter()
                .any(|u| transport_state(u) == ConnState::Connected)
        );
    }

    #[test]
    fn a_business_code_is_a_device_refusal_not_a_transport_failure() {
        assert!(is_device_refusal(&ApiError::Api {
            code: "1006".into(),
            message: "device does not exist".into()
        }));
        assert!(!is_device_refusal(&ApiError::Malformed("http 500".into())));
        assert_eq!(
            ApiError::Malformed("http 429".into()).http_status(),
            Some(429)
        );
        assert_eq!(
            ApiError::Api {
                code: "0".into(),
                message: String::new()
            }
            .http_status(),
            None
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
        assert!(out.iter().any(|u| matches!(u, Update::Notification(_))));
        assert_eq!(s.state(), ConnState::AuthError);
        let words = out
            .iter()
            .filter_map(|u| match u {
                Update::Notification(n) => Some(n.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(words.contains("sign-in failed"), "plain words: {words}");
        assert!(
            !words.contains("key "),
            "no key material in the message: {words}"
        );
    }

    #[test]
    fn a_reconnect_is_counted_once_per_recovery() {
        let mut s = LiveState::new();
        let list = vec![DiscoveredDevice {
            sn: "AAA".into(),
            product_name: "DELTA 2 Max".into(),
            online: true,
            product_type: Some(81),
        }];
        s.record_list(now(), &list);
        assert_eq!(s.reconnects, 0, "the first connection is not a reconnect");
        s.record_failure(now() + chrono::Duration::minutes(5), &failure());
        s.record_sweep(now() + chrono::Duration::minutes(10), 1, 0, 0);
        assert_eq!(s.reconnects, 1, "one recovery is one reconnect");
    }

    /// The state carried by a `Transport` update.
    fn transport_state(u: &Update) -> ConnState {
        match u {
            Update::Transport(_, info) => info.state,
            other => panic!("expected a transport update, got {other:?}"),
        }
    }

    #[allow(dead_code)]
    fn unused_json(v: serde_json::Value) {
        let _ = json!(v);
    }
}
