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
    /// A transport's state, with the facts behind it: the last success, the last
    /// failure, the status code, the consecutive failures, the reconnects.
    Transport(Transport, TransportInfo),
    /// The cloud refused one device. The code and the message are what the API
    /// returned, verbatim: a refusal is a device problem, not a connection problem.
    DeviceRefusal {
        sn: String,
        code: String,
        message: String,
    },
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
    /// The launch update check's result for this run. The network lives in
    /// `src/update/`; the UI receives only this, so the UI never calls the API.
    Release(crate::update::Status),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Rest,
    Mqtt,
}

impl Transport {
    /// The word a person uses for the path: the cloud API, and the live data feed.
    pub fn label(&self) -> &'static str {
        match self {
            Transport::Rest => "cloud",
            Transport::Mqtt => "live data",
        }
    }
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
    /// Plain words for the header. "DEGRADED" does not tell a person what to do.
    pub fn label(&self) -> &'static str {
        match self {
            ConnState::Initializing => "starting",
            ConnState::Connecting => "connecting",
            ConnState::Connected => "online",
            ConnState::Degraded => "problems",
            ConnState::Reconnecting => "reconnecting",
            ConnState::Offline => "offline",
            ConnState::AuthError => "sign-in failed",
        }
    }
}

/// What a transport is doing, and what it last did. No secrets: the status code and
/// the server's own words, never a key, a nonce, or a signature.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportInfo {
    pub state: ConnState,
    /// The last request that came back good.
    pub last_ok: Option<DateTime<Utc>>,
    /// The last request that failed, with the code the server answered with.
    pub last_fail: Option<DateTime<Utc>>,
    pub status: Option<u16>,
    pub error: Option<String>,
    /// Failures in a row. A success resets it.
    pub fails: u32,
    /// Reconnections this session: MQTT `CONNACK`s, REST sessions re-opened.
    pub reconnects: u32,
}

impl TransportInfo {
    pub fn new(state: ConnState) -> Self {
        Self {
            state,
            last_ok: None,
            last_fail: None,
            status: None,
            error: None,
            fails: 0,
            reconnects: 0,
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
