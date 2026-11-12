use chrono::{DateTime, Utc};

use crate::services::{ConnState, Transport, TransportInfo};

/// REST and MQTT are tracked independently: MQTT can be down while REST works, and
/// the app keeps running on cached state. A refusal of one device never enters this
/// state, because it is a device problem and the transport answered normally.
#[derive(Debug, Clone)]
pub struct ConnectionManager {
    rest: TransportInfo,
    mqtt: TransportInfo,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self {
            rest: TransportInfo::new(ConnState::Initializing),
            mqtt: TransportInfo::new(ConnState::Initializing),
        }
    }
}

impl ConnectionManager {
    /// Store what the service observed. The service owns the state machine; the
    /// manager keeps the facts for the UI.
    pub fn set(&mut self, transport: Transport, info: TransportInfo, _now: DateTime<Utc>) {
        match transport {
            Transport::Rest => self.rest = info,
            Transport::Mqtt => self.mqtt = info,
        }
    }

    pub fn rest(&self) -> ConnState {
        self.rest.state
    }

    pub fn mqtt(&self) -> ConnState {
        self.mqtt.state
    }

    pub fn rest_info(&self) -> &TransportInfo {
        &self.rest
    }

    pub fn mqtt_info(&self) -> &TransportInfo {
        &self.mqtt
    }

    /// The header shows one overall state: online if any transport is connected,
    /// problems if one is connected and the other is not, offline if neither is.
    pub fn overall(&self) -> ConnState {
        if self.rest.state == ConnState::AuthError || self.mqtt.state == ConnState::AuthError {
            return ConnState::AuthError;
        }
        match (self.rest.state, self.mqtt.state) {
            (ConnState::Connected, ConnState::Connected) => ConnState::Connected,
            (ConnState::Connected, ConnState::Offline)
            | (ConnState::Offline, ConnState::Connected) => ConnState::Degraded,
            (ConnState::Offline, ConnState::Offline) => ConnState::Offline,
            (ConnState::Reconnecting, ConnState::Connected)
            | (ConnState::Connected, ConnState::Reconnecting) => ConnState::Degraded,
            _ => ConnState::Reconnecting,
        }
    }

    /// Exponential backoff with a ceiling, so retries do not hammer the API.
    /// Base 2 s, capped at 60 s; MQTT reconnect uses the same schedule.
    /// Exponential backoff with a ceiling, so retries do not hammer the API: 1 s,
    /// doubling, capped at 32 s. MQTT reconnect uses the same schedule.
    pub fn backoff_secs(&self) -> u64 {
        1u64 << self.rest.fails.max(self.mqtt.fails).min(5)
    }

    pub fn is_stale(&self, now: DateTime<Utc>, window_secs: i64) -> bool {
        let latest = [self.rest.last_ok, self.mqtt.last_ok]
            .into_iter()
            .flatten()
            .max();
        match latest {
            None => true,
            Some(t) => (now - t).num_seconds() > window_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn one_live_transport_is_not_reported_as_offline() {
        let mut m = ConnectionManager::default();
        m.set(
            Transport::Rest,
            TransportInfo {
                state: ConnState::Offline,
                fails: 3,
                last_fail: Some(now()),
                status: Some(500),
                ..TransportInfo::new(ConnState::Offline)
            },
            now(),
        );
        m.set(
            Transport::Mqtt,
            TransportInfo::new(ConnState::Connected),
            now(),
        );
        let mut mqtt = TransportInfo::new(ConnState::Connected);
        mqtt.last_ok = Some(now());
        m.set(Transport::Mqtt, mqtt, now());
        assert_eq!(m.overall(), ConnState::Degraded, "one side is working");
        assert!(!m.is_stale(now(), 120), "the live feed is fresh");
    }

    #[test]
    fn backoff_grows_with_failures_and_is_capped() {
        let mut m = ConnectionManager::default();
        let mut info = TransportInfo::new(ConnState::Reconnecting);
        for n in 1..=6 {
            info.fails = n;
            m.set(Transport::Rest, info.clone(), now());
        }
        assert_eq!(m.backoff_secs(), 32, "the ceiling is 2^5 s");
        let mut ok = TransportInfo::new(ConnState::Connected);
        ok.last_ok = Some(now() - Duration::seconds(30));
        m.set(Transport::Rest, ok, now());
        assert_eq!(m.backoff_secs(), 1, "a success resets the schedule");
    }

    #[test]
    fn no_activity_at_all_is_stale() {
        let m = ConnectionManager::default();
        assert!(m.is_stale(now(), 60));
    }
}
