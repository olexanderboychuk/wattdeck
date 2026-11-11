pub mod connection_manager;
pub mod live_provider;
pub mod telemetry_service;

pub use connection_manager::ConnectionManager;

use chrono::{DateTime, Utc};

use crate::telemetry::models::DeviceSnapshot;

/// Messages the UI receives from the background services. Bounded channel, one message
/// per device update; the UI coalesces by sequence number.
#[derive(Debug, Clone)]
pub enum Update {
    /// Full device list as discovered.
    DeviceList(Vec<DiscoveredDevice>),
    /// A merged device snapshot after MQTT/REST/mock data.
    Snapshot(Box<DeviceSnapshot>),
    /// Connection state of a transport, as observed by the service.
    Transport(Transport, ConnState),
    /// A user-visible failure: auth error, rate limit, parse error.
    Notification(Notification),
    /// Samples read back from the history database, oldest first.
    History(String, Vec<crate::telemetry::history::Sample>),
    /// The device's answer to a command: the code the reply carries (`0` accepted,
    /// `-1` not owned, `-2` offline) and its message.
    ControlReply {
        sn: String,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Rest,
    Mqtt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Initializing,
    Connecting,
    Connected,
    Degraded,
    Reconnecting,
    Offline,
    AuthError,
}

impl ConnState {
    pub fn label(&self) -> &'static str {
        match self {
            ConnState::Initializing => "INIT",
            ConnState::Connecting => "CONNECT",
            ConnState::Connected => "ONLINE",
            ConnState::Degraded => "DEGRADED",
            ConnState::Reconnecting => "RECONNECT",
            ConnState::Offline => "OFFLINE",
            ConnState::AuthError => "AUTH ERR",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredDevice {
    pub sn: String,
    pub product_name: String,
    pub online: bool,
    /// Product type is known from MQTT quota payloads and from the mock provider.
    /// The REST device list does not include it, so it stays `None` until quota data
    /// arrives.
    pub product_type: Option<u16>,
}

/// Commands from the UI to the background services. The UI never calls the API
/// directly; it asks a service to do the work.
#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
    Scenario(Scenario),
    /// A control the user confirmed: serial, the control, the value to send.
    Control {
        sn: String,
        control: &'static crate::controls::Control,
        value: i64,
    },
    Quit,
}

use crate::services::telemetry_service::Scenario;

#[derive(Debug, Clone)]
pub struct Notification {
    pub level: Level,
    pub text: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Critical,
}

impl Notification {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: Level::Info,
            text: text.into(),
            at: Utc::now(),
        }
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            text: text.into(),
            at: Utc::now(),
        }
    }
    pub fn critical(text: impl Into<String>) -> Self {
        Self {
            level: Level::Critical,
            text: text.into(),
            at: Utc::now(),
        }
    }
}
