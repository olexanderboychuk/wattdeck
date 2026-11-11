use chrono::{DateTime, Utc};

use crate::services::{ConnState, Transport};

/// REST and MQTT are tracked independently: MQTT can be down while REST works, and
/// the app keeps running on cached state.
#[derive(Debug, Clone)]
pub struct ConnectionManager {
    rest: ConnState,
    mqtt: ConnState,
    pub last_rest_ok: Option<DateTime<Utc>>,
    pub last_mqtt_ok: Option<DateTime<Utc>>,
    consecutive_failures: u32,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self {
            rest: ConnState::Initializing,
            mqtt: ConnState::Initializing,
            last_rest_ok: None,
            last_mqtt_ok: None,
            consecutive_failures: 0,
        }
    }
}

impl ConnectionManager {
    pub fn set(&mut self, transport: Transport, state: ConnState, now: DateTime<Utc>) {
        match transport {
            Transport::Rest => {
                self.rest = state;
                if state == ConnState::Connected {
                    self.last_rest_ok = Some(now);
                    self.consecutive_failures = 0;
                }
            }
            Transport::Mqtt => {
                self.mqtt = state;
                if state == ConnState::Connected {
                    self.last_mqtt_ok = Some(now);
                    self.consecutive_failures = 0;
                }
            }
        }
        if state == ConnState::Offline || state == ConnState::AuthError {
            self.consecutive_failures += 1;
        }
    }

    pub fn rest(&self) -> ConnState {
        self.rest
    }

    pub fn mqtt(&self) -> ConnState {
        self.mqtt
    }

    /// The header shows one overall state: online if any transport is connected,
    /// degraded if one is connected and the other is not, offline if neither is.
    pub fn overall(&self) -> ConnState {
        if self.rest == ConnState::AuthError || self.mqtt == ConnState::AuthError {
            return ConnState::AuthError;
        }
        match (self.rest, self.mqtt) {
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
    pub fn backoff_secs(&self) -> u64 {
        let n = self.consecutive_failures.min(6);
        2u64 << n
    }

    pub fn is_stale(&self, now: DateTime<Utc>, window_secs: i64) -> bool {
        let latest = [self.last_rest_ok, self.last_mqtt_ok]
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

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn starts_initializing() {
        let m = ConnectionManager::default();
        assert_eq!(m.rest(), ConnState::Initializing);
        assert_eq!(m.overall(), ConnState::Reconnecting);
    }

    #[test]
    fn mqtt_down_with_rest_up_is_degraded() {
        let mut m = ConnectionManager::default();
        m.set(Transport::Rest, ConnState::Connected, now());
        m.set(Transport::Mqtt, ConnState::Offline, now());
        assert_eq!(m.overall(), ConnState::Degraded);
    }

    #[test]
    fn both_connected_is_online() {
        let mut m = ConnectionManager::default();
        m.set(Transport::Rest, ConnState::Connected, now());
        m.set(Transport::Mqtt, ConnState::Connected, now());
        assert_eq!(m.overall(), ConnState::Connected);
    }

    #[test]
    fn both_offline_is_offline() {
        let mut m = ConnectionManager::default();
        m.set(Transport::Rest, ConnState::Offline, now());
        m.set(Transport::Mqtt, ConnState::Offline, now());
        assert_eq!(m.overall(), ConnState::Offline);
    }

    #[test]
    fn auth_error_wins() {
        let mut m = ConnectionManager::default();
        m.set(Transport::Rest, ConnState::AuthError, now());
        m.set(Transport::Mqtt, ConnState::Connected, now());
        assert_eq!(m.overall(), ConnState::AuthError);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let mut m = ConnectionManager::default();
        assert_eq!(m.backoff_secs(), 2);
        m.set(Transport::Rest, ConnState::Offline, now());
        assert_eq!(m.backoff_secs(), 4);
        for _ in 0..10 {
            m.set(Transport::Mqtt, ConnState::Offline, now());
        }
        assert_eq!(m.backoff_secs(), 128, "must stay bounded");
    }

    #[test]
    fn staleness_tracks_last_success() {
        let mut m = ConnectionManager::default();
        assert!(m.is_stale(now(), 60));
        m.set(Transport::Mqtt, ConnState::Connected, now());
        assert!(!m.is_stale(now(), 60));
    }
}
