use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS, SubscribeReasonCode, Transport};
use tokio::sync::mpsc::Sender;

use serde_json::Value;

use crate::api::client::Client;
use crate::api::devices;
use crate::api::devices::MqttCredentials;
use crate::config::settings::MqttConfig;
use crate::services::{
    Command, ConnState, DiscoveredDevice, Notification, Transport as Source, TransportInfo, Update,
};
use crate::telemetry::models::{DeviceSnapshot, Source as Data};
use crate::telemetry::normalizer::{merge_params, renorm};
use crate::utils::formatting::mask_serial;

/// A stable client id. The broker tolerates about 10 unique ids per day per key, so
/// the id is derived from the account name and stays the same across runs; an
/// override in the config wins.
pub fn client_id(cfg: &MqttConfig, account: &str) -> String {
    if let Some(id) = &cfg.client_id {
        return id.clone();
    }
    let mut h: u64 = 0x5366;
    for b in account.bytes() {
        h = h.wrapping_mul(2862933555777941587).wrapping_add(b as u64);
    }
    format!("ef-tui-{:08x}", h % 0x1_0000_0000)
}

/// The connection parameters, as they will be applied to the broker. Split out of
/// `options` so the rule is testable: `MqttOptions` keeps its fields private.
#[derive(Debug, Clone, PartialEq)]
pub struct BrokerPlan {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub keepalive_secs: u16,
    pub clean_session: bool,
}

pub fn plan(cfg: &MqttConfig, creds: &MqttCredentials) -> BrokerPlan {
    BrokerPlan {
        host: cfg
            .host_override
            .clone()
            .unwrap_or_else(|| creds.host.clone()),
        port: creds.port,
        tls: cfg.tls_override.unwrap_or(creds.tls),
        // The docs give 15–60 s; the broker disconnects a client that stays silent
        // past the keepalive, so the value is floored at 15.
        keepalive_secs: cfg.keepalive_secs.max(15),
        // Clean session: the broker keeps no state for this client, so a reconnect
        // re-subscribes from scratch.
        clean_session: cfg.reconnect,
    }
}

pub fn options(cfg: &MqttConfig, creds: &MqttCredentials, id: &str) -> MqttOptions {
    let p = plan(cfg, creds);
    let mut o = MqttOptions::new(id, p.host, p.port);
    o.set_keep_alive(Duration::from_secs(p.keepalive_secs as u64));
    o.set_clean_session(p.clean_session);
    o.set_credentials(&creds.username, &creds.password);
    if p.tls {
        o.set_transport(Transport::tls_with_default_config());
    }
    o
}

/// The live MQTT source: connect to the broker the certification endpoint reports,
/// subscribe the reply topics one packet at a time, publish `latestQuotas` on the
/// refresh cadence, and merge the partial blocks the device pushes.
pub struct MqttProvider {
    cfg: MqttConfig,
    api: Client,
    devices: Vec<DiscoveredDevice>,
    snapshots: HashMap<String, DeviceSnapshot>,
    /// Per-serial last reported failure, so one refused device is not re-alerted on
    /// every message.
    last_error: HashMap<String, String>,
    /// The ids of the commands this session has published, so a `set_reply` naming
    /// an id can be reported as the control it answers.
    sent_ids: HashMap<u64, &'static crate::controls::Control>,
    /// The MQTT username (`certificateAccount`), the first path element of the
    /// developer topics. It is a credential, so it is never printed.
    account: String,
    /// The id the next command carries. A live DELTA 2 Max answers a `set` by
    /// echoing this id on `set_reply`, so each command gets its own.
    next_cmd_id: u64,
    /// What the MQTT transport has done, for the diagnostics screen: reconnects, the
    /// last good message, the last failure.
    conn: TransportInfo,
}

impl MqttProvider {
    pub fn new(cfg: MqttConfig, api: Client) -> Self {
        Self {
            cfg,
            api,
            devices: Vec::new(),
            snapshots: HashMap::new(),
            last_error: HashMap::new(),
            sent_ids: HashMap::new(),
            account: String::new(),
            next_cmd_id: crate::controls::BASE_ID,
            conn: TransportInfo::new(ConnState::Initializing),
        }
    }

    /// The MQTT state to report, with the counters kept here: a `CONNACK` after a
    /// drop is one reconnect, a broker error is one failure.
    fn mqtt_state(&mut self, next: ConnState, now: DateTime<Utc>) -> Update {
        if next == ConnState::Connected {
            if self.conn.state != ConnState::Connected && self.conn.state != ConnState::Initializing
            {
                self.conn.reconnects += 1;
            }
            self.conn.last_ok = Some(now);
            self.conn.fails = 0;
        } else if next == ConnState::Reconnecting {
            self.conn.last_fail = Some(now);
            self.conn.fails += 1;
        }
        self.conn.state = next;
        Update::Transport(Source::Mqtt, self.conn.clone())
    }

    pub async fn run(mut self, tx: Sender<Update>, mut cmd: tokio::sync::mpsc::Receiver<Command>) {
        let list = match devices::device_list(&self.api).await {
            Ok(l) => l,
            Err(e) => {
                let _ = tx
                    .send(Update::Notification(Notification::critical(format!(
                        "MQTT discovery: {e}"
                    ))))
                    .await;
                let _ = tx
                    .send(self.mqtt_state(ConnState::AuthError, Utc::now()))
                    .await;
                return;
            }
        };
        self.devices = list.clone();
        let _ = tx.send(Update::DeviceList(list)).await;

        let creds = match devices::mqtt_credentials(&self.api).await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(Update::Notification(Notification::critical(format!(
                        "MQTT credentials: {e}"
                    ))))
                    .await;
                let _ = tx
                    .send(self.mqtt_state(ConnState::AuthError, Utc::now()))
                    .await;
                return;
            }
        };
        self.account = creds.username.clone();
        let id = client_id(&self.cfg, &creds.username);
        let opts = options(&self.cfg, &creds, &id);
        let (client, mut ev) = AsyncClient::new(opts, 64);
        let refresh = Duration::from_secs(self.cfg.refresh_interval_seconds.clamp(20, 60));
        let mut timer = tokio::time::interval(refresh);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let _ = tx
            .send(self.mqtt_state(ConnState::Connecting, Utc::now()))
            .await;

        loop {
            tokio::select! {
                ev = ev.poll() => {
                    match ev {
                        Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                            let _ = tx
                                .send(self.mqtt_state(ConnState::Connected, Utc::now()))
                                .await;
                            // Each topic goes out as its own SUBSCRIBE packet: a
                            // combined subscribe is refused by the broker.
                            for d in self.devices.clone() {
                                for t in crate::mqtt::topics::subscribe_topics(&self.account, &d.sn) {
                                    // The topic carries the serial, so it is masked in the log.
                                    tracing::debug!("subscribe {}", t.replace(&d.sn, &mask_serial(&d.sn)));
                                    if client.subscribe(t, QoS::AtLeastOnce).await.is_err() {
                                        let _ = tx.try_send(Update::Notification(Notification::warn(
                                            "MQTT subscribe failed",
                                        )));
                                    }
                                }
                            }
                            self.publish(&client, &tx).await;
                        }
                        Ok(Event::Incoming(Incoming::Publish(p))) => {
                            // In debug runs the payload is echoed with the serial masked, so the
                            // real message shapes can be checked against the parser.
                            if tracing::enabled!(tracing::Level::DEBUG) {
                                let mut text = String::from_utf8_lossy(&p.payload).to_string();
                                for d in &self.devices {
                                    text = text.replace(&d.sn, &mask_serial(&d.sn));
                                }
                                tracing::debug!("{} bytes on {}: {text}", p.payload.len(), p.topic.replace(&self.account, "acct"));
                            }
                            self.ingest(&p.topic, &p.payload, &tx);
                        }
                        Ok(Event::Incoming(Incoming::SubAck(sa))) => {
                            tracing::debug!("SUBACK pkid {} codes {:?}", sa.pkid, sa.return_codes);
                            // 0x80 means the broker refused that topic.
                            if sa.return_codes.iter().any(|c| matches!(c, SubscribeReasonCode::Failure)) {
                                let _ = tx.try_send(Update::Notification(Notification::warn(
                                    "MQTT subscription refused by the broker",
                                )));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let _ = tx
                                .send(self.mqtt_state(ConnState::Reconnecting, Utc::now()))
                                .await;
                            let _ = tx.try_send(Update::Notification(Notification::warn(format!(
                                "MQTT: {e}"
                            ))));
                            // rumqttc reconnects inside `poll`; this pause keeps a
                            // failing broker from spinning the loop.
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
                _ = timer.tick() => {
                    self.publish(&client, &tx).await;
                }
                // A command the user confirmed in the UI. It goes out on the verified
                // set topic, QoS 1, and the device answers on `set_reply`.
                c = cmd.recv() => {
                    let quit = matches!(c, Some(Command::Quit));
                    if let Some(Command::Control {
                        sn,
                        control,
                        value,
                    }) = c
                    {
                        let raw = self
                            .snapshots
                            .get(&sn)
                            .map(|s| s.raw.clone())
                            .unwrap_or_default();
                        match command_plan(&sn, control, value, self.next_cmd_id, &raw) {
                            Ok(payload) => {
                                self.sent_ids.insert(self.next_cmd_id, control);
                                self.next_cmd_id += 1;
                                let topic = crate::mqtt::topics::set_topic(&self.account, &sn);
                                // The topic carries the serial, so it is masked in the log.
                                tracing::debug!(
                                    "publish {} on {}",
                                    payload,
                                    topic.replace(&sn, &mask_serial(&sn))
                                );
                                if client
                                    .publish(topic, QoS::AtLeastOnce, false, payload.into_bytes())
                                    .await
                                    .is_err()
                                {
                                    let _ = tx.try_send(Update::ControlReply {
                                        sn,
                                        code: "-4".to_string(),
                                        message: "the broker refused the publish".to_string(),
                                    });
                                }
                            }
                            Err((code, message)) => {
                                let _ = tx.try_send(Update::ControlReply { sn, code, message });
                            }
                        }
                    }
                    if quit {
                        return;
                    }
                }
            }
        }
    }

    /// Ask every device for its latest snapshot. Devices throttle their pushes when
    /// nobody polls, so this runs on the refresh cadence.
    async fn publish(&mut self, client: &AsyncClient, tx: &Sender<Update>) {
        let mut refused = false;
        for d in &self.devices {
            let topic = crate::mqtt::topics::refresh_topic(&self.account, &d.sn);
            if client
                .publish(
                    topic,
                    QoS::AtLeastOnce,
                    false,
                    crate::mqtt::payload::latest_quotas(),
                )
                .await
                .is_err()
            {
                refused = true;
            }
        }
        if refused {
            let _ = tx.try_send(self.mqtt_state(ConnState::Degraded, Utc::now()));
        }
    }

    /// One MQTT payload merged into the accumulated snapshot for its serial.
    fn ingest(&mut self, topic: &str, payload: &[u8], tx: &Sender<Update>) {
        let Some(sn) = crate::mqtt::topics::sn_of(&self.account, topic) else {
            return;
        };
        let p = crate::mqtt::payload::parse(payload);
        // The device's answer to a command goes to the app as a reply, so the
        // confirmation dialog can report what happened. `code` is the envelope's
        // string: 0 accepted, -1 not owned, -2 offline.
        if crate::mqtt::topics::kind_of(topic) == Some("set_reply") {
            // The reply echoes the command id, so the control it answers is known:
            // the alert can name the control, not just a code.
            let label =
                p.id.and_then(|id| self.sent_ids.remove(&id))
                    .map(|c| c.label);
            let code = p.code.unwrap_or_else(|| "no code".to_string());
            let message = match (label, code.as_str()) {
                (Some(l), "0") => format!("{l} accepted by the device"),
                (Some(l), _) => format!("{l} refused: {}", p.message.unwrap_or_default()),
                (None, "0") => "accepted by the device".to_string(),
                (None, _) => p.message.unwrap_or_default(),
            };
            let _ = tx.try_send(Update::ControlReply { sn, code, message });
            return;
        }
        if p.keys.is_empty() {
            if let Some(on) = p.online {
                if let Some(s) = self.snapshots.get_mut(&sn) {
                    s.online = on;
                    if let Some(v) = p.status_raw {
                        // Kept for the diagnostics view; the UI shows unhandled keys.
                        s.raw
                            .insert("mqtt.status".to_string(), serde_json::json!(v));
                    }
                    s.last_update = Utc::now();
                    let _ = tx.try_send(Update::Snapshot(Box::new(s.clone())));
                }
                return;
            }
            if let Some(code) = p.code.filter(|c| c != "0") {
                let text = format!(
                    "{}: code {code} {}",
                    mask_serial(&sn),
                    p.message.unwrap_or_default()
                );
                if report_once(&mut self.last_error, &sn, &text) {
                    let _ = tx.try_send(Update::Notification(Notification::warn(text)));
                }
            }
            return;
        }
        if !self.snapshots.contains_key(&sn) {
            let (name, ptype) = self
                .devices
                .iter()
                .find(|d| d.sn == sn)
                .map(|d| (d.product_name.clone(), d.product_type.unwrap_or(0)))
                .unwrap_or((String::new(), 0));
            self.snapshots
                .insert(sn.clone(), DeviceSnapshot::new(sn.clone(), name, ptype));
        }
        let snap = self.snapshots.get_mut(&sn).expect("just inserted");
        merge_params(snap, &p.keys);
        renorm(snap);
        snap.source = Data::Mqtt;
        snap.seq += 1;
        snap.last_update = Utc::now();
        // Telemetry streamed through the broker, so the device is reachable, whatever
        // the `online` field in the snapshot envelope says: a live DELTA 2 Max reports
        // `online: 0` in `get_reply` while it is publishing. The device list's offline
        // flag stays in force until data arrives.
        snap.online = true;
        self.last_error.remove(&sn);
        let _ = tx.try_send(Update::Snapshot(Box::new(snap.clone())));
    }
}

/// A confirmed command, turned into the payload to publish, or the reason nothing is
/// sent. A partial param set was published by this app on 2026-11-10, half-applied by
/// a live DELTA 2 Max (`inv.SlowChgWatts` moved to a value that was never sent), and
/// never answered. The full param set the verified commands carry is answered with
/// code 0, so the params travel, each echoed from the value the device reports. A
/// device that has reported nothing cannot be echoed, so the command is not sent.
fn command_plan(
    sn: &str,
    control: &crate::controls::Control,
    value: i64,
    id: u64,
    raw: &HashMap<String, Value>,
) -> Result<String, (String, String)> {
    if !control.accepts(value) {
        return Err((
            "-3".to_string(),
            format!("value {value} is outside the range the device accepts"),
        ));
    }
    if raw.is_empty() {
        return Err((
            "-5".to_string(),
            format!(
                "{} has not reported any settings, nothing was sent",
                mask_serial(sn)
            ),
        ));
    }
    Ok(crate::controls::set_payload(control, value, id, raw))
}

/// Record a per-device failure and report whether it is new. The same refusal from the
/// same device is not a new alert.
fn report_once(last: &mut HashMap<String, String>, sn: &str, text: &str) -> bool {
    if last.get(sn) == Some(&text.to_string()) {
        return false;
    }
    last.insert(sn.to_string(), text.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::credentials::Credentials;
    use crate::config::settings::ApiConfig;
    use crate::config::settings::MqttConfig;
    use crate::devices::registry::Family;

    /// The live property map this account's DELTA 2 Max reports.
    fn live_raw() -> HashMap<String, Value> {
        serde_json::from_str(include_str!("../../tests/fixtures/quota_all.json")).expect("fixture")
    }

    fn creds() -> MqttCredentials {
        MqttCredentials {
            username: "open-testacct".to_string(),
            password: "secret".to_string(),
            host: "mqtt-e.ecoflow.com".to_string(),
            port: 8883,
            tls: true,
        }
    }

    #[test]
    fn the_client_id_is_stable_across_runs() {
        let cfg = MqttConfig::default();
        let a = client_id(&cfg, "open-testacct");
        let b = client_id(&cfg, "open-testacct");
        assert_eq!(a, b, "the broker allows few unique ids per day");
        assert!(a.starts_with("ef-tui-"));
        assert_ne!(a, client_id(&cfg, "open-other"));
        // An explicit id in the config wins.
        let cfg2 = MqttConfig {
            client_id: Some("mine".to_string()),
            ..MqttConfig::default()
        };
        assert_eq!(client_id(&cfg2, "open-testacct"), "mine");
    }

    #[test]
    fn options_use_the_reported_broker_and_tls() {
        let cfg = MqttConfig::default();
        let p = plan(&cfg, &creds());
        assert_eq!(p.host, "mqtt-e.ecoflow.com");
        assert_eq!(p.port, 8883);
        assert!(p.tls, "the certification endpoint reports protocol mqtts");
        // 30 s keepalive, inside the 15–60 s range the docs describe.
        assert_eq!(p.keepalive_secs, 30);
        assert!(p.clean_session);
        let low = MqttConfig {
            keepalive_secs: 1,
            ..MqttConfig::default()
        };
        assert_eq!(plan(&low, &creds()).keepalive_secs, 15);
        let over = MqttConfig {
            host_override: Some("localhost".to_string()),
            tls_override: Some(false),
            ..MqttConfig::default()
        };
        let p = plan(&over, &creds());
        assert_eq!(p.host, "localhost");
        assert!(!p.tls, "plain TCP on a local broker for tests");
        let o = options(&over, &creds(), "id1");
        assert_eq!(o.client_id(), "id1");
        assert!(o.clean_session());
    }

    #[test]
    fn a_real_get_reply_is_ingested_into_the_snapshot() {
        // The verbatim DELTA 2 Max `get_reply` capture, ingested the way the provider
        // ingests a broker message.
        let topic = "/open/acct/R351ZEB5PGC90303/get_reply";
        let payload = br#"{"id":874179349,"version":"1.0","operateType":"latestQuotas","moduleSn":"R351ZEB5PGC90303","code":"0","data":{"sn":"R351ZEB5PGC90303","productType":81,"model":1,"online":0,"quotaMap":{"bms_bmsStatus.soc":85,"bms_bmsStatus.soh":99,"bms_bmsStatus.amp":-5183,"bms_bmsStatus.vol":52985,"bms_bmsStatus.remainCap":33549,"bms_bmsStatus.designCap":40000,"pd.wattsInSum":224,"pd.wattsOutSum":226,"inv.outputWatts":226,"pd.usb1Watts":18}}}"#;
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut p = MqttProvider::new(
            MqttConfig::default(),
            Client::new(&ApiConfig::default(), &Credentials::default()),
        );
        p.account = "acct".to_string();
        p.devices = vec![DiscoveredDevice {
            sn: "R351ZEB5PGC90303".to_string(),
            product_name: "DELTA 2 Max".to_string(),
            online: false,
            product_type: Some(81),
        }];
        p.ingest(topic, payload, &tx);
        let u = rx.blocking_recv().expect("snapshot");
        let snap = match u {
            Update::Snapshot(s) => *s,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        assert_eq!(snap.source, Data::Mqtt);
        // Telemetry streamed, so the device is reachable even though the envelope
        // said `online: 0`.
        assert!(snap.online);
        assert_eq!(snap.battery.soc, Some(85.0));
        assert_eq!(snap.battery.soh, Some(99.0));
        // 52.985 V × 33.549 Ah, derived from the raw capacities.
        assert!((snap.battery.remaining_wh.unwrap() - 1777.94).abs() < 0.5);
        assert_eq!(snap.power.total_in_w, Some(224.0));
        assert_eq!(snap.power.ac_out_w, Some(226.0));
    }

    #[test]
    fn a_refused_device_is_warned_about_once() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut p = MqttProvider::new(
            MqttConfig::default(),
            Client::new(&ApiConfig::default(), &Credentials::default()),
        );
        p.account = "acct".to_string();
        p.ingest(
            "/open/acct/RB2MN5ZE7721004/get_reply",
            br#"{"code":"-1","message":"device is not owned"}"#,
            &tx,
        );
        let u = rx.blocking_recv().expect("notification");
        let text = match u {
            Update::Notification(n) => n.text,
            other => panic!("expected a notification, got {other:?}"),
        };
        assert!(
            text.starts_with("RB***********04"),
            "the serial is masked: {text}"
        );
        // The same refusal must not fill the alert list.
        p.ingest(
            "/open/acct/RB2MN5ZE7721004/get_reply",
            br#"{"code":"-1","message":"device is not owned"}"#,
            &tx,
        );
        assert!(rx.is_empty(), "the refusal was reported once");
    }
    #[test]
    fn a_set_reply_is_reported_as_the_control_it_answers() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut p = MqttProvider::new(
            MqttConfig::default(),
            Client::new(&ApiConfig::default(), &Credentials::default()),
        );
        p.account = "acct".to_string();
        // The id the provider recorded for the command it sent, and the answer this
        // account's DELTA 2 Max gave on 2026-11-10, which echoes that id.
        p.sent_ids.insert(
            999900001,
            crate::controls::for_family(Family::Delta2)
                .iter()
                .find(|c| c.id == "charge_limit")
                .expect("charge limit"),
        );
        p.ingest(
            "/open/acct/R351ZEB5PGC90303/set_reply",
            br#"{"code":"0","moduleType":2,"data":{"ack":0},"operateType":"upsConfig","version":"1.0","id":999900001,"time":632391642}"#,
            &tx,
        );
        match rx.blocking_recv().expect("reply") {
            Update::ControlReply { sn, code, message } => {
                assert_eq!(sn, "R351ZEB5PGC90303");
                assert_eq!(code, "0");
                assert!(
                    message.contains("charge limit"),
                    "the reply names the control it answers: {message}"
                );
            }
            other => panic!("expected a command reply, got {other:?}"),
        }
        // A reply whose id the session did not send keeps the device's own words.
        p.ingest(
            "/open/acct/R351ZEB5PGC90303/set_reply",
            br#"{"code":"-2","message":"device offline"}"#,
            &tx,
        );
        match rx.blocking_recv().expect("reply") {
            Update::ControlReply { code, message, .. } => {
                assert_eq!(code, "-2");
                assert_eq!(message, "device offline");
            }
            other => panic!("expected a command reply, got {other:?}"),
        }
    }

    #[test]
    fn a_command_carries_the_params_the_device_answers() {
        // The app published a partial param set on 2026-11-10: the broker accepted it,
        // the device half-applied it (`inv.SlowChgWatts` moved to a value that was
        // never sent) and never answered. The full param set is answered with code 0.
        let raw = live_raw();
        let c = crate::controls::for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_charge_watts")
            .expect("ac charge watts");
        let payload = command_plan("R351ZEB5PGC90303", c, 900, 999900001, &raw).expect("payload");
        let p: Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(p["params"]["slowChgWatts"], 900, "the picked value");
        assert_eq!(p["params"]["fastChgWatts"], 2400, "echoed from the device");
        assert_eq!(p["params"]["chgPauseFlag"], 0, "echoed from the device");
    }

    #[test]
    fn a_device_that_has_reported_nothing_is_not_commanded() {
        // With nothing to echo, the command would carry invented values for the
        // settings the verified command includes.
        let raw = HashMap::new();
        let c = crate::controls::for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        let (code, message) =
            command_plan("R351ZEB5PGC90303", c, 0, 999900001, &raw).expect_err("refused");
        assert_eq!(code, "-5");
        assert!(
            message.contains("R3************03"),
            "the message names the masked serial: {message}"
        );
        assert!(
            !message.contains("R351ZEB5PGC90303"),
            "the plain serial is not in the message"
        );
    }

    #[test]
    fn a_value_the_device_does_not_accept_is_refused_before_publishing() {
        let raw = live_raw();
        let c = crate::controls::for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "charge_limit")
            .expect("charge limit");
        let (code, message) =
            command_plan("R351ZEB5PGC90303", c, 40, 999900001, &raw).expect_err("refused");
        assert_eq!(code, "-3");
        assert!(
            message.contains("40"),
            "the message names the value: {message}"
        );
    }

    #[test]
    fn repeated_device_failures_are_reported_once() {
        let mut last = HashMap::new();
        let e = "R3************44: code -1 device is not owned".to_string();
        assert!(report_once(&mut last, "R351ZEB5PGC90303", &e));
        assert!(
            !report_once(&mut last, "R351ZEB5PGC90303", &e),
            "the same refusal is not a new alert"
        );
        assert!(
            report_once(&mut last, "RB2MN5ZE7721004", &e),
            "a second device is its own alert"
        );
    }
}
