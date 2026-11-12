use std::collections::{BTreeMap, HashMap, VecDeque};

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::config::settings::TempUnit;
use crate::devices::registry::{Family, lookup, lookup_by_name};
use crate::input::Action;
use crate::input::keybindings::KeyMap;
use crate::services::telemetry_service::Scenario;
use crate::services::{
    Command, ConnState, ConnectionManager, DiscoveredDevice, Level, Notification, Transport, Update,
};
use crate::storage::{StorageCmd, StorageHandle};
use crate::telemetry::history::{History, Sample};
use crate::telemetry::models::{DeviceSnapshot, Source};
use crate::telemetry::normalizer::renorm;
use crate::ui::theme::Theme;
use crate::utils::formatting::mask_serial;

/// Analytics time ranges. Ranges the in-memory window cannot cover are filled from
/// the history database.
/// Minimum spacing between two database queries for the same range.
pub const QUERY_THROTTLE_SECS: i64 = 5;

/// A station that is doing nothing sends the same numbers every sample. Identical
/// samples are not stored; the last identical sample is stored again this often, so
/// history says the station is still alive.
pub const HEARTBEAT_SECS: i64 = 600;

/// The facts alerts are evaluated from: serial, SOC, temperature, total output
/// power, online. Grouped so the evaluation loop stays readable.
type SnapshotFacts = (String, Option<f64>, Option<f64>, Option<f64>, bool);

pub const RANGES: [(&str, i64); 6] = [
    ("15 min", 900),
    ("1 hour", 3600),
    ("6 hours", 21600),
    ("24 hours", 86400),
    ("7 days", 604800),
    ("30 days", 2592000),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Devices,
    DeviceDetails,
    Analytics,
    Alerts,
    Settings,
    Help,
}

impl Screen {
    pub fn label(&self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Devices => "Devices",
            Screen::DeviceDetails => "Device",
            Screen::Analytics => "Analytics",
            Screen::Alerts => "Alerts",
            Screen::Settings => "Settings",
            Screen::Help => "Help",
        }
    }

    /// The tab strip order.
    pub fn strip() -> [Screen; 5] {
        [
            Screen::Dashboard,
            Screen::Devices,
            Screen::Analytics,
            Screen::Alerts,
            Screen::Settings,
        ]
    }

    pub fn is_strip(&self) -> bool {
        matches!(
            self,
            Screen::Dashboard
                | Screen::Devices
                | Screen::Analytics
                | Screen::Alerts
                | Screen::Settings
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    BatteryCritical,
    BatteryLow,
    HighTemp,
    DeviceOffline,
    DeviceOnline,
    StorageBehind,
    HighOutput,
    Setting,
    Command,
    CommandReply,
    TransportDown,
    TransportUp,
    AuthError,
    /// The credential state of the run: which backend the keys came from, and
    /// the mock-mode hint naming the wizard.
    Credential,
}

#[derive(Debug, Clone)]
pub struct Alert {
    pub sn: String,
    pub kind: AlertKind,
    pub level: Level,
    pub text: String,
    pub at: DateTime<Utc>,
}

pub struct App {
    pub config: Config,
    pub keys: KeyMap,
    pub devices: Vec<DeviceSnapshot>,
    pub selected: usize,
    pub screen: Screen,
    pub panel: usize,
    pub connection: ConnectionManager,
    pub alerts: VecDeque<Alert>,
    pub notices: VecDeque<Notification>,
    pub now: DateTime<Utc>,
    pub running: bool,
    pub mock: bool,
    pub scenario: Scenario,
    pub status: String,
    pub theme: Theme,
    /// Analytics time range index.
    pub range: usize,
    /// UI-to-service channel. The UI never contacts the API directly.
    pub command_tx: Option<tokio::sync::mpsc::Sender<Command>>,
    history: HashMap<String, History>,
    last_sample: HashMap<String, DateTime<Utc>>,
    /// The sample last stored per device, so an unchanged report is not stored twice.
    last_same: HashMap<String, Sample>,
    last_online: HashMap<String, bool>,
    emitted: HashMap<(String, AlertKind), DateTime<Utc>>,
    /// Samples read back from the history database, per device, oldest first.
    pub history_cache: HashMap<String, Vec<crate::telemetry::history::Sample>>,
    /// The storage writer, when history is enabled. The app only sends rows.
    storage: Option<StorageHandle>,
    /// Commands the app has asked for, recorded in tests so the confirmation flow
    /// can be checked without a services channel.
    #[cfg(test)]
    sent_commands: Vec<Command>,
    /// Tells the running loop the tick length changed, after an fps key.
    tick_notify: Option<tokio::sync::mpsc::Sender<std::time::Duration>>,
    /// The control the user is about to send, before the confirmation dialog.
    pub pending_control: Option<crate::controls::Pending>,
    pub control_row: usize,
    /// Cap on cached rows per device, so a query for 30 days cannot grow memory.
    query_limit: usize,
    /// One-shot flag for the "writer behind" notice, so a saturated queue is
    /// reported once and not on every dropped sample.
    history_behind_reported: bool,
    /// Last database query issued: range and instant, so a range switch asks for
    /// history once per range and repeated keypresses do not queue a read each time.
    last_query: Option<(usize, DateTime<Utc>)>,
    pub device_filter: String,
}

impl App {
    pub fn new(config: Config, mock: bool, scenario: Scenario) -> Self {
        let keys = KeyMap::from_config(&config.keys);
        let color_mode = config.ui.color_mode;
        Self {
            config,
            keys,
            devices: Vec::new(),
            selected: 0,
            screen: Screen::Dashboard,
            panel: 0,
            connection: ConnectionManager::default(),
            alerts: VecDeque::new(),
            notices: VecDeque::new(),
            now: Utc::now(),
            running: true,
            mock,
            scenario,
            status: String::new(),
            theme: Theme::new(color_mode),
            range: 2,
            command_tx: None,
            history: HashMap::new(),
            last_sample: HashMap::new(),
            last_same: HashMap::new(),
            last_online: HashMap::new(),
            emitted: HashMap::new(),
            device_filter: String::new(),
            history_cache: HashMap::new(),
            storage: None,
            tick_notify: None,
            #[cfg(test)]
            sent_commands: Vec::new(),
            pending_control: None,
            control_row: 0,
            query_limit: 4096,
            history_behind_reported: false,
            last_query: None,
        }
    }

    pub fn set_storage(&mut self, handle: crate::storage::StorageHandle) {
        self.storage = Some(handle);
    }

    pub fn set_tick_notifier(&mut self, tx: tokio::sync::mpsc::Sender<std::time::Duration>) {
        self.tick_notify = Some(tx);
    }

    pub fn set_command_sender(&mut self, tx: tokio::sync::mpsc::Sender<Command>) {
        self.command_tx = Some(tx);
    }

    /// Fire-and-forget to the services. A full channel means the service is behind;
    /// dropping the command is the right behaviour, not a queue that grows.
    fn send_command(&mut self, cmd: Command) {
        #[cfg(test)]
        self.sent_commands.push(cmd.clone());
        if let Some(tx) = &self.command_tx {
            let _ = tx.try_send(cmd);
        }
    }

    /// The commands the app has asked the services to send, for the tests.
    #[cfg(test)]
    pub fn sent_commands(&self) -> &[Command] {
        &self.sent_commands
    }

    pub fn selected_device(&self) -> Option<&DeviceSnapshot> {
        self.devices.get(self.selected)
    }

    pub fn history_for(&self, sn: &str) -> &History {
        static EMPTY: History = History::new(0);
        self.history.get(sn).unwrap_or(&EMPTY)
    }

    pub fn visible_devices(&self) -> Vec<usize> {
        self.devices
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                self.device_filter.is_empty()
                    || d.product_name
                        .to_lowercase()
                        .contains(&self.device_filter.to_lowercase())
                    || d.masked_serial().contains(&self.device_filter)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Apply one message from a background service. This runs on the UI thread and
    /// never performs network work.
    pub fn apply_update(&mut self, update: Update) {
        match update {
            Update::DeviceList(list) => {
                for d in list {
                    self.register_device(&d);
                }
            }
            Update::Snapshot(snap) => {
                let sn = snap.sn.clone();
                match self.devices.iter().position(|d| d.sn == sn) {
                    Some(i) => {
                        // REST reports the whole property map every poll, MQTT pushes
                        // partial blocks. Merge by key so a partial block cannot erase
                        // what the other source reported, then re-derive the fields.
                        let d = &mut self.devices[i];
                        for (k, v) in &snap.raw {
                            d.raw.insert(k.clone(), v.clone());
                        }
                        d.source = snap.source;
                        d.online = snap.online;
                        d.seq = snap.seq;
                        d.last_update = snap.last_update;
                        // Which transport answered, so the device screen can show the
                        // last message from each.
                        match snap.source {
                            Source::Mqtt => d.last_mqtt = Some(snap.last_update),
                            Source::Rest => d.last_rest = Some(snap.last_update),
                            _ => {}
                        }
                        if snap.product_type != 0 {
                            d.product_type = snap.product_type;
                        }
                        // The family comes from the product type, and the key tables
                        // are per family. A partial block (an MQTT `status` push) carries
                        // no product type, so its snapshot is `Unknown`; that must not
                        // erase the family the device is known to be, which would blank
                        // every field the model-specific table fills.
                        if snap.family != Family::Unknown {
                            d.family = snap.family;
                        }
                        renorm(d);
                        tracing::debug!(
                            "{} raw {} keys cap {:?} soc {:?} out {:?}",
                            mask_serial(&sn),
                            d.raw.len(),
                            d.battery.capacity_wh,
                            d.battery.soc,
                            d.power.total_out_w
                        );
                    }
                    None => {
                        let mut d = *snap;
                        // The first snapshot for a serial is inserted as it arrives.
                        // Derived figures (Wh capacities, charge direction, port list)
                        // are computed from the merged raw keys, so the very first
                        // reading is complete, not just the fields the source set.
                        renorm(&mut d);
                        self.devices.push(d);
                        self.history
                            .entry(sn.clone())
                            .or_insert_with(|| History::new(self.config.history.memory_samples));
                    }
                }
                self.maybe_sample(&sn);
                self.evaluate_alerts();
            }
            Update::DeviceRefusal { sn, code, message } => {
                // The cloud answered and refused this one device, so the connection
                // stays as it is; only this device's data is unavailable.
                if let Some(d) = self.devices.iter_mut().find(|d| d.sn == sn) {
                    d.last_refusal = Some(format!("code {code}: {message}"));
                }
            }
            Update::Transport(transport, info) => {
                let prev = match transport {
                    Transport::Rest => self.connection.rest(),
                    Transport::Mqtt => self.connection.mqtt(),
                };
                let state = info.state;
                self.connection.set(transport, info, self.now);
                if prev != ConnState::Connected && state == ConnState::Connected {
                    self.push_alert(
                        AlertKind::TransportUp,
                        Level::Info,
                        "",
                        format!("{} is online", transport.label()),
                    );
                }
                if prev == ConnState::Connected && state != ConnState::Connected {
                    self.push_alert(
                        AlertKind::TransportDown,
                        Level::Warn,
                        "",
                        format!("{}: {}", transport.label(), state.label()),
                    );
                }
                if state == ConnState::AuthError {
                    self.push_alert(
                        AlertKind::AuthError,
                        Level::Critical,
                        "",
                        "sign-in failed. Check your API credentials.",
                    );
                }
            }
            Update::ControlReply { sn, code, message } => {
                // The device's answer to a command the user confirmed. The code is
                // what the reply envelope carries: `0` accepted, `-1` not owned,
                // `-2` offline.
                let level = if code == "0" {
                    Level::Info
                } else {
                    Level::Warn
                };
                self.push_alert(
                    AlertKind::CommandReply,
                    level,
                    &sn,
                    format!("command reply code {code} {message}"),
                );
            }
            Update::History(sn, rows) => {
                // Query answers land in a bounded cache, oldest-first, so the analytics
                // ranges the in-memory window cannot cover are filled from the database.
                let cache = self.history_cache.entry(sn.clone()).or_default();
                *cache = rows;
                if cache.len() > self.query_limit {
                    cache.truncate(self.query_limit);
                }
            }
            Update::Notification(n) => {
                self.notices.push_back(n);
                if self.notices.len() > 200 {
                    self.notices.pop_front();
                }
            }
        }
    }

    fn register_device(&mut self, d: &DiscoveredDevice) {
        if self.devices.iter().any(|x| x.sn == d.sn) {
            return;
        }
        let info = d
            .product_type
            .map(lookup)
            .unwrap_or_else(|| lookup_by_name(&d.product_name));
        let mut snap = DeviceSnapshot::new(d.sn.clone(), d.product_name.clone(), info.product_type);
        snap.family = info.family;
        snap.online = d.online;
        self.devices.push(snap);
        self.history
            .entry(d.sn.clone())
            .or_insert_with(|| History::new(self.config.history.memory_samples));
        if let Some(store) = &self.storage {
            let _ = store.send(StorageCmd::Device {
                sn: self.store_key(&d.sn),
                model: d.product_name.clone(),
                seen: self.now,
            });
        }
    }

    /// Push a sample on the configured interval, not on every MQTT message.
    fn maybe_sample(&mut self, sn: &str) {
        if !self.config.history.enabled {
            return;
        }
        let last = self
            .last_sample
            .get(sn)
            .copied()
            .unwrap_or(DateTime::UNIX_EPOCH);
        if (self.now - last).num_seconds() < self.config.history.sample_interval_seconds as i64 {
            return;
        }
        let Some(d) = self.devices.iter().find(|d| d.sn == sn) else {
            return;
        };
        // Sampling needs a measurement. A device that has reported nothing is not a
        // device measuring zero, so nothing is stored.
        let Some(sample) = Sample::from_device(self.now, d) else {
            return;
        };
        let unchanged = self
            .last_same
            .get(sn)
            .map(|last| last.same_measurement(&sample))
            .unwrap_or(false);
        // An identical measurement is stored again only on the heartbeat, so history
        // records that the station is still answering without filling the database
        // with the same reading every interval.
        if unchanged && (self.now - last).num_seconds() < HEARTBEAT_SECS {
            return;
        }
        self.last_same.insert(sn.to_string(), sample);
        self.history
            .entry(sn.to_string())
            .or_insert_with(|| History::new(self.config.history.memory_samples))
            .push(sample);
        self.last_sample.insert(sn.to_string(), self.now);
        // The same row goes to the database. A full writer queue drops it: the live
        // view is never held up by history, and the drop is reported once.
        if let Some(store) = &self.storage
            && !store.send(StorageCmd::Sample {
                sn: self.store_key(sn),
                sample,
            })
            && !self.history_behind_reported
        {
            self.history_behind_reported = true;
            self.push_alert(
                AlertKind::StorageBehind,
                Level::Warn,
                "",
                "history writer is behind",
            );
        }
    }

    /// Alert evaluation with per-(device, kind) cooldown so a flat battery does not
    /// generate a stream of the same alert.
    fn evaluate_alerts(&mut self) {
        let cooldown = self.config.alerts.cooldown_seconds as i64;
        let snapshot: Vec<SnapshotFacts> = self
            .devices
            .iter()
            .map(|d| {
                (
                    d.sn.clone(),
                    d.battery.soc,
                    d.battery.max_cell_temp_c.or(d.battery.temp_c),
                    d.power.total_out_w,
                    d.online,
                )
            })
            .collect();

        for (sn, soc, temp, out_w, online) in snapshot {
            // Online/offline transitions are edge-triggered, and the offline notice
            // is the one the user can switch off: a device that is simply unplugged
            // should not fill the alert list. The state is tracked either way.
            let prev = self.last_online.insert(sn.clone(), online);
            if self.config.alerts.notify_disconnected {
                if prev == Some(true) && !online {
                    self.push_alert(
                        AlertKind::DeviceOffline,
                        Level::Warn,
                        &sn,
                        "device went offline",
                    );
                }
                if prev == Some(false) && online {
                    self.push_alert(
                        AlertKind::DeviceOnline,
                        Level::Info,
                        &sn,
                        "device back online",
                    );
                }
            }

            if let Some(soc) = soc {
                if soc <= self.config.alerts.battery_critical_percent as f64 {
                    self.push_alert(
                        AlertKind::BatteryCritical,
                        Level::Critical,
                        &sn,
                        format!("battery {:.0}% (critical)", soc),
                    );
                } else if soc <= self.config.alerts.battery_low_percent as f64 {
                    self.push_alert(
                        AlertKind::BatteryLow,
                        Level::Warn,
                        &sn,
                        format!("battery {:.0}% (low)", soc),
                    );
                }
            }
            if let Some(t) = temp
                && t >= self.config.alerts.high_temperature_c
            {
                self.push_alert(
                    AlertKind::HighTemp,
                    Level::Warn,
                    &sn,
                    format!("temperature {:.0} °C", t),
                );
            }
            if let Some(w) = out_w
                && w >= self.config.alerts.high_output_w
            {
                self.push_alert(
                    AlertKind::HighOutput,
                    Level::Warn,
                    &sn,
                    format!(
                        "output {:.0} W, above the {:.0} W limit set in the config",
                        w, self.config.alerts.high_output_w
                    ),
                );
            }
        }

        // Suppress repeated alerts inside the cooldown window.
        let mut seen: HashMap<(String, AlertKind), DateTime<Utc>> = HashMap::new();
        let keep: Vec<Alert> = self
            .alerts
            .iter()
            .rev()
            .filter(|a| {
                let key = (a.sn.clone(), a.kind);
                match seen.get(&key) {
                    Some(t) => (a.at - *t).num_seconds() > cooldown,
                    None => {
                        seen.insert(key, a.at);
                        true
                    }
                }
            })
            .rev()
            .cloned()
            .collect();
        self.alerts = VecDeque::from(keep);
        self.emitted = seen;
    }

    /// A state the app wants seen, not a device event: the alert goes on the
    /// Alerts screen under `system`. Only the TUI attaches storage, and a mock
    /// run never does, so a mock-mode alert cannot write history.
    pub fn system_alert(&mut self, kind: AlertKind, level: Level, text: impl Into<String>) {
        self.push_alert(kind, level, "", text);
    }

    fn push_alert(&mut self, kind: AlertKind, level: Level, sn: &str, text: impl Into<String>) {
        let text = text.into();
        let alert = Alert {
            sn: if sn.is_empty() {
                "system".to_string()
            } else {
                self.mask(sn)
            },
            kind,
            level,
            text: text.clone(),
            at: self.now,
        };
        self.alerts.push_back(alert);
        if self.alerts.len() > 500 {
            self.alerts.pop_front();
        }
        // Events are part of the history. They are keyed by the masked serial
        // whatever the UI shows, so switching the serial display on cannot put a
        // plain serial in the file.
        if let Some(store) = &self.storage {
            let _ = store.send(StorageCmd::Event {
                t: self.now,
                level: format!("{level:?}"),
                kind: format!("{kind:?}"),
                sn: if sn.is_empty() {
                    "system".to_string()
                } else {
                    self.store_key(sn)
                },
                text,
            });
        }
    }

    /// The key history is stored under. Always masked, whatever the UI shows: the
    /// database is a file on disk, and a plain serial has no place in it.
    fn store_key(&self, sn: &str) -> String {
        mask_serial(sn)
    }

    fn mask(&self, sn: &str) -> String {
        if self.config.ui.show_serial {
            sn.to_string()
        } else {
            crate::utils::formatting::mask_serial(sn)
        }
    }

    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Dashboard => self.screen = Screen::Dashboard,
            Action::Devices => self.screen = Screen::Devices,
            Action::Analytics => self.screen = Screen::Analytics,
            Action::Alerts => self.screen = Screen::Alerts,
            Action::Settings => self.screen = Screen::Settings,
            Action::Help => self.screen = Screen::Help,
            Action::NextScreen => self.cycle_screen(1),
            Action::PrevScreen => self.cycle_screen(-1),
            Action::NextPanel => self.panel = self.panel.saturating_add(1) % self.panel_count(),
            Action::PrevPanel => {
                let n = self.panel_count();
                self.panel = (self.panel + n - 1) % n;
            }
            Action::Up | Action::PageUp => self.move_selection(-1),
            Action::Down | Action::PageDown => self.move_selection(1),
            Action::Home => self.selected = 0,
            Action::End => self.selected = self.devices.len().saturating_sub(1),
            Action::Enter => {
                if self.pending_control.is_some() {
                    // The dialog is open: Enter is the confirmation key.
                    self.confirm_control();
                } else if self.screen == Screen::Devices {
                    self.screen = Screen::DeviceDetails;
                    // The controls live on this screen; ask for a control directly.
                    self.request_control();
                } else if self.screen == Screen::DeviceDetails {
                    self.request_control();
                }
            }
            Action::Confirm => {
                if self.pending_control.is_some() {
                    self.confirm_control();
                }
            }
            Action::Cancel | Action::Escape => {
                if self.pending_control.is_some() {
                    // Cancel is always safe: the command is dropped, nothing is sent.
                    self.pending_control = None;
                    self.status = "cancelled, nothing was sent".to_string();
                } else if self.screen == Screen::DeviceDetails {
                    self.screen = Screen::Devices;
                } else if self.screen == Screen::Help {
                    self.screen = Screen::Dashboard;
                }
            }
            Action::Right => {
                if self.screen == Screen::DeviceDetails {
                    self.adjust_control(1);
                } else if self.screen == Screen::Analytics {
                    self.range = (self.range + 1).min(RANGES.len() - 1);
                    if let Some(d) = self.selected_device() {
                        let sn = d.sn.clone();
                        self.request_history(&sn);
                    }
                } else {
                    self.panel = self.panel.saturating_add(1) % self.panel_count();
                }
            }
            Action::Left => {
                if self.screen == Screen::DeviceDetails {
                    self.adjust_control(-1);
                } else if self.screen == Screen::Analytics {
                    self.range = self.range.saturating_sub(1);
                    if let Some(d) = self.selected_device() {
                        let sn = d.sn.clone();
                        self.request_history(&sn);
                    }
                } else {
                    let n = self.panel_count();
                    self.panel = (self.panel + n - 1) % n;
                }
            }
            Action::NextControl | Action::PrevControl => {
                let controls = self.controls_for(self.selected);
                if !controls.is_empty() && self.config.controls.enabled {
                    let n = controls.len() as i64;
                    let d = if action == Action::NextControl { 1 } else { -1 };
                    self.control_row = (((self.control_row as i64 + d) % n + n) % n) as usize;
                    let c = controls[self.control_row];
                    self.status = format!(
                        "control {} = {}",
                        c.label,
                        c.render(self.control_value(c).unwrap_or(0))
                    );
                }
            }
            Action::Refresh => {
                self.status = "refresh requested".to_string();
                self.send_command(Command::Refresh);
            }
            Action::Quit => {
                self.running = false;
                self.send_command(Command::Quit);
            }
            Action::CycleScenario => {
                if self.mock {
                    self.scenario = self.scenario.next();
                    self.status = format!("mock scenario: {}", self.scenario.label());
                    self.send_command(Command::Scenario(self.scenario));
                }
            }
            Action::ToggleSerial => self.config.ui.show_serial = !self.config.ui.show_serial,
            Action::ToggleTempUnit => {
                self.config.ui.temperature_unit = match self.config.ui.temperature_unit {
                    TempUnit::Celsius => TempUnit::Fahrenheit,
                    TempUnit::Fahrenheit => TempUnit::Celsius,
                };
            }
            Action::CycleFps => {
                // A small set of sensible rates, so a keypress cannot ask for a
                // frame rate the terminal will not keep up with.
                self.config.ui.fps = match self.config.ui.fps {
                    15 => 30,
                    30 => 60,
                    _ => 15,
                };
                self.push_alert(
                    AlertKind::Setting,
                    Level::Info,
                    "",
                    format!("frame rate {} fps", self.config.ui.fps),
                );
                // The frame rate decides the tick length, so the running loop needs
                // to be told.
                if let Some(tx) = &self.tick_notify {
                    let _ = tx.try_send(self.config.tick_interval());
                }
            }
        }
    }

    fn panel_count(&self) -> usize {
        match self.screen {
            Screen::Dashboard => 3,
            Screen::Devices => 2,
            Screen::DeviceDetails => 4,
            Screen::Analytics => 2,
            Screen::Alerts => 1,
            Screen::Settings => 1,
            Screen::Help => 1,
        }
    }

    fn cycle_screen(&mut self, delta: i32) {
        let strip = Screen::strip();
        let idx = strip
            .iter()
            .position(|s| core::mem::discriminant(s) == core::mem::discriminant(&self.screen))
            .unwrap_or(0);
        let next = (idx as i32 + delta).rem_euclid(strip.len() as i32) as usize;
        self.screen = strip[next];
        self.panel = 0;
    }

    fn move_selection(&mut self, delta: i32) {
        if self.devices.is_empty() {
            return;
        }
        let visible = self.visible_devices();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        let next = (pos as i32 + delta).rem_euclid(visible.len() as i32) as usize;
        self.selected = visible[next];
    }

    pub fn tick(&mut self, now: DateTime<Utc>) {
        self.now = now;
    }

    /// Seconds since the freshest device update, for the "UPDATED" header field.
    pub fn freshness(&self) -> Option<f64> {
        self.devices
            .iter()
            .map(|d| d.seconds_since_update(self.now))
            .min_by(|a, b| a.partial_cmp(b).unwrap())
    }

    pub fn stale(&self) -> bool {
        self.connection
            .is_stale(self.now, self.stale_window_secs() as i64)
    }

    /// The window past which a device's last measurement counts as stale: twice the
    /// cloud refresh interval, so one missed poll is not an alarm.
    pub fn stale_window_secs(&self) -> f64 {
        self.config.api.refresh_interval_seconds as f64 * 2.0
    }

    /// The source of the newest device data. A degraded transport that is still
    /// delivering is shown as the source; the per-transport states next to it in the
    /// header carry the quality detail.
    pub fn source_label(&self) -> String {
        if self.mock {
            return "MOCK".to_string();
        }
        fn rank(s: Source) -> u8 {
            match s {
                Source::Mqtt => 3,
                Source::Rest => 2,
                Source::Mock => 1,
                Source::None => 0,
            }
        }
        let best = self
            .devices
            .iter()
            .map(|d| d.source)
            .max_by_key(|s| rank(*s))
            .unwrap_or(Source::None);
        match best {
            Source::Mqtt => "MQTT".to_string(),
            Source::Rest => "REST".to_string(),
            Source::Mock => "MOCK".to_string(),
            Source::None => {
                // No device data yet: label the transport that is closest to working.
                fn rank(c: ConnState) -> u8 {
                    match c {
                        ConnState::Connected => 4,
                        ConnState::Degraded => 3,
                        ConnState::Reconnecting => 2,
                        ConnState::Connecting | ConnState::Initializing => 1,
                        ConnState::AuthError | ConnState::Offline => 0,
                    }
                }
                let m = self.connection.mqtt();
                let r = self.connection.rest();
                if rank(m) >= 3 && rank(m) >= rank(r) {
                    "MQTT".to_string()
                } else if rank(r) >= 3 {
                    "REST".to_string()
                } else {
                    "OFFLINE".to_string()
                }
            }
        }
    }

    pub fn alert_count(&self) -> usize {
        self.alerts.len()
    }

    pub fn unacknowledged_criticals(&self) -> usize {
        self.alerts
            .iter()
            .filter(|a| a.level == Level::Critical)
            .count()
    }

    /// Samples inside the current analytics range.
    pub fn range_samples(&self, sn: &str) -> Vec<crate::telemetry::history::Sample> {
        let secs = RANGES[self.range].1;
        let cutoff = self.now - chrono::Duration::seconds(secs);
        // In-memory samples plus the rows the database query brought back. The same
        // instant can be in both, so rows are keyed by time and sorted.
        let mut merged: BTreeMap<i64, Sample> = BTreeMap::new();
        for s in self.history_for(sn).ordered() {
            merged.insert(s.t.timestamp(), s);
        }
        // The cache is keyed by the masked serial, the key the database uses.
        if let Some(rows) = self.history_cache.get(&self.store_key(sn)) {
            for s in rows {
                merged.insert(s.t.timestamp(), *s);
            }
        }
        merged.values().filter(|s| s.t >= cutoff).copied().collect()
    }

    /// Ask the writer for the history of one range. Ranges the merged samples
    /// already cover need no query, so the database is only read when it helps.
    pub fn request_history(&mut self, sn: &str) {
        if !self.config.history.enabled {
            return;
        }
        let Some(store) = &self.storage else {
            return;
        };
        if self.range_covered(sn) {
            return;
        }
        // Throttle: hammering the range selector must not queue one read per
        // keypress. The same range is asked at most once every few seconds.
        if let Some((range, at)) = self.last_query
            && range == self.range
            && (self.now - at).num_seconds() < QUERY_THROTTLE_SECS
        {
            return;
        }
        let secs = RANGES[self.range].1;
        let from = self.now - chrono::Duration::seconds(secs);
        if store.send(StorageCmd::Query {
            sn: self.store_key(sn),
            from,
            to: self.now,
            limit: self.query_limit,
        }) {
            self.last_query = Some((self.range, self.now));
        }
    }

    /// Whether the samples in hand (memory plus database) cover the requested
    /// range. Ten minutes of span is the minimum that makes a longer range worth
    /// showing; a range the data cannot span is reported as partial.
    pub fn range_covered(&self, sn: &str) -> bool {
        let secs = RANGES[self.range].1;
        let samples = self.range_samples(sn);
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return false;
        };
        (last.t - first.t).num_seconds() >= secs.min(600)
    }

    /// Rows the database has brought back for a device.
    pub fn db_rows(&self, sn: &str) -> usize {
        self.history_cache
            .get(&self.store_key(sn))
            .map(|r| r.len())
            .unwrap_or(0)
    }

    /// Whether the history writer is running.
    pub fn history_stored(&self) -> bool {
        self.storage.is_some()
    }

    /// The in-memory window in minutes: samples per device times the sampling rate.
    pub fn window_minutes(&self) -> i64 {
        self.config.history.memory_samples as i64
            * self.config.history.sample_interval_seconds as i64
            / 60
    }

    /// The controls this device accepts: the family's verified command list, with
    /// the ones the device does not report removed. Empty means nothing is offered.
    pub fn controls_for(&self, index: usize) -> Vec<&'static crate::controls::Control> {
        let Some(d) = self.devices.get(index) else {
            return Vec::new();
        };
        // The family the snapshot carries is the one the data was read with: the
        // product type from the device list, or the `productType` the snapshot
        // reported. A device discovered without a product type is not assumed to be
        // a DELTA 2, so it gets no commands.
        let info = lookup(d.product_type);
        if d.family == Family::Unknown && info.family == Family::Unknown {
            return Vec::new();
        }
        crate::controls::for_family(d.family)
            .iter()
            .filter(|c| c.available(info.caps))
            .collect()
    }

    /// The value a control currently has, as the device reported it.
    pub fn control_value(&self, control: &crate::controls::Control) -> Option<i64> {
        let d = self.selected_device()?;
        d.raw.get(control.read_key).and_then(|v| v.as_i64())
    }

    /// Step the selected control's candidate value by `delta` (one row of the list
    /// is selected at a time; a toggle flips, a number moves by its own step).
    pub fn adjust_control(&mut self, delta: i64) {
        if self.selected_device().is_none() {
            return;
        }
        let controls = self.controls_for(self.selected);
        if controls.is_empty() || !self.config.controls.enabled {
            return;
        }
        let row = self.control_row.min(controls.len() - 1);
        let c = controls[row];
        let current = self.control_value(c).unwrap_or(match c.kind {
            crate::controls::ValueKind::Toggle => 0,
            crate::controls::ValueKind::Number { min, .. } => min,
        });
        let step = match c.kind {
            crate::controls::ValueKind::Toggle => 1,
            crate::controls::ValueKind::Number { unit, .. } => match unit {
                "%" => 5,
                "W" => 100,
                "min" => 60,
                _ => 30,
            },
        };
        let next = (current + delta * step).clamp(
            match c.kind {
                crate::controls::ValueKind::Toggle => 0,
                crate::controls::ValueKind::Number { min, .. } => min,
            },
            match c.kind {
                crate::controls::ValueKind::Toggle => 1,
                crate::controls::ValueKind::Number { max, .. } => max,
            },
        );
        self.pending_control = Some(crate::controls::Pending {
            control: c,
            value: next,
        });
    }

    /// Open the confirmation dialog for the selected control. Nothing is sent here.
    pub fn request_control(&mut self) {
        if !self.config.controls.enabled {
            self.status = "controls are off: set [controls] enabled = true".to_string();
            return;
        }
        if self.selected_device().is_none() {
            return;
        }
        let controls = self.controls_for(self.selected);
        if controls.is_empty() {
            self.status = "this product has no verified controls".to_string();
            return;
        }
        let row = self.control_row.min(controls.len() - 1);
        let c = controls[row];
        let value = self.control_value(c).unwrap_or(match c.kind {
            crate::controls::ValueKind::Toggle => 1,
            crate::controls::ValueKind::Number { min, .. } => min,
        });
        self.pending_control = Some(crate::controls::Pending { control: c, value });
        self.status = format!("confirm {} = {}", c.label, c.render(value));
    }

    /// The confirmation key: the command goes out now, to the MQTT provider.
    pub fn confirm_control(&mut self) {
        let Some(pending) = self.pending_control.take() else {
            return;
        };
        let Some(d) = self.devices.get(self.selected) else {
            return;
        };
        // A device that is not reporting is not sent a command.
        if !d.online {
            self.pending_control = Some(pending);
            self.status = "the device is not reporting; nothing was sent".to_string();
            return;
        }
        let sn = d.sn.clone();
        self.push_alert(
            AlertKind::Command,
            Level::Info,
            &sn,
            format!(
                "sending {} = {}",
                pending.control.label,
                pending.control.render(pending.value)
            ),
        );
        self.send_command(Command::Control {
            sn,
            control: pending.control,
            value: pending.value,
        });
        // The footer says what left, so the user can see the command was not dropped.
        self.status = format!(
            "sending {} = {}",
            pending.control.label,
            pending.control.render(pending.value)
        );
    }
    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.device_filter = filter.into();
        self.selected = 0;
    }

    /// Rows for the CONTROLS panel: each control the device accepts, with the value
    /// the device reported, and the candidate value the keys are moving.
    pub fn control_rows(&self) -> Vec<(String, String, String, bool)> {
        if self.selected_device().is_none() {
            return Vec::new();
        }
        let controls = self.controls_for(self.selected);
        if controls.is_empty() {
            return Vec::new();
        }
        let row = self.control_row.min(controls.len() - 1);
        controls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let reported = self
                    .control_value(c)
                    .map(|v| c.render(v))
                    .unwrap_or_else(|| "--".to_string());
                let candidate = if i == row {
                    self.pending_control
                        .as_ref()
                        .filter(|p| p.control.id == c.id)
                        .map(|p| c.render(p.value))
                        .unwrap_or_else(|| "no change".to_string())
                } else {
                    String::new()
                };
                (c.label.to_string(), reported, candidate, i == row)
            })
            .collect()
    }

    /// The dialog text, when a command is waiting for confirmation.
    pub fn control_dialog(&self) -> Option<String> {
        let p = self.pending_control.as_ref()?;
        let d = self.devices.get(self.selected)?;
        Some(format!(
            "Send {} to {} ?  {} = {}   enter/y = send, n/esc = cancel",
            p.control.label,
            self.mask(&d.sn),
            p.control.param,
            p.control.render(p.value)
        ))
    }
}

impl Source {
    pub fn is_mock(&self) -> bool {
        *self == Source::Mock
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::settings::HistoryConfig;
    use crate::services::TransportInfo;
    use crate::telemetry::models::DeviceState;
    use crate::utils::formatting::temp;

    fn app() -> App {
        App::new(Config::default(), true, Scenario::SolarCharging)
    }

    /// A snapshot built the way a provider builds one: property keys through the
    /// normalizer, so the app's merge and re-derive path is what the test exercises.
    /// The control settings are included, because the live device reports them.
    fn snap(sn: &str, soc: f64, in_w: f64, out_w: f64) -> DeviceSnapshot {
        let mut s = DeviceSnapshot::new(sn, "DELTA 2 Max", 81);
        let mut keys = HashMap::new();
        keys.insert("bms_bmsStatus.soc".to_string(), serde_json::json!(soc));
        keys.insert("pd.wattsInSum".to_string(), serde_json::json!(in_w));
        keys.insert("pd.wattsOutSum".to_string(), serde_json::json!(out_w));
        for (k, v) in [
            ("bms_emsStatus.maxChargeSoc", serde_json::json!(100)),
            ("bms_emsStatus.minDsgSoc", serde_json::json!(5)),
            ("inv.cfgAcEnabled", serde_json::json!(1)),
            ("pd.dcOutState", serde_json::json!(1)),
            ("inv.SlowChgWatts", serde_json::json!(400)),
            ("pd.standbyMin", serde_json::json!(30)),
            ("pd.beepMode", serde_json::json!(0)),
            ("pd.lcdOffSec", serde_json::json!(300)),
        ] {
            keys.insert(k.to_string(), v);
        }
        crate::telemetry::normalizer::merge_params(&mut s, &keys);
        s.source = Source::Mock;
        s.online = true;
        s
    }

    #[test]
    fn device_list_registers_devices() {
        let mut a = app();
        a.apply_update(Update::DeviceList(vec![
            DiscoveredDevice {
                sn: "AAA".into(),
                product_name: "DELTA 2 Max".into(),
                online: true,
                product_type: Some(81),
            },
            DiscoveredDevice {
                sn: "BBB".into(),
                product_name: "RIVER 2 Pro".into(),
                online: false,
                product_type: Some(72),
            },
        ]));
        assert_eq!(a.devices.len(), 2);
        assert_eq!(a.devices[0].product_name, "DELTA 2 Max");
    }

    #[test]
    fn a_full_property_map_fills_every_field() {
        // A REST snapshot is the whole 240-key map; the app must merge it and re-derive.
        let mut a = app();
        let raw = serde_json::from_str::<serde_json::Value>(include_str!(
            "../tests/fixtures/quota_all.json"
        ))
        .expect("fixture");
        let mut snap = DeviceSnapshot::new("R351ZEB5PGC90303", "DELTA 2 Max", 81);
        for (k, v) in raw.as_object().unwrap() {
            crate::telemetry::normalizer::apply_key(&mut snap, k, v);
        }
        snap.source = Source::Rest;
        snap.online = true;
        a.apply_update(Update::Snapshot(Box::new(snap)));
        let d = &a.devices[0];
        assert_eq!(d.battery.soc, Some(85.0));
        assert_eq!(d.battery.soh, Some(99.0));
        assert!((d.battery.capacity_wh.unwrap() - 2119.4).abs() < 1.0);
        // 52.985 V × 33.549 Ah, derived from the raw capacities.
        assert!((d.battery.remaining_wh.unwrap() - 1777.94).abs() < 0.5);
    }

    #[test]
    fn a_partial_block_keeps_what_the_full_map_reported() {
        // MQTT pushes one changed block; the merged snapshot must keep the rest.
        let mut a = app();
        let raw = serde_json::from_str::<serde_json::Value>(include_str!(
            "../tests/fixtures/quota_all.json"
        ))
        .expect("fixture");
        let mut full = DeviceSnapshot::new("R351ZEB5PGC90303", "DELTA 2 Max", 81);
        for (k, v) in raw.as_object().unwrap() {
            crate::telemetry::normalizer::apply_key(&mut full, k, v);
        }
        full.source = Source::Rest;
        a.apply_update(Update::Snapshot(Box::new(full)));
        // The live pdStatus block, verbatim shape: keys prefixed by the typeCode.
        let mut block = DeviceSnapshot::new("R351ZEB5PGC90303", "DELTA 2 Max", 81);
        crate::telemetry::normalizer::apply_key(
            &mut block,
            "pd.wattsOutSum",
            &serde_json::json!(245),
        );
        crate::telemetry::normalizer::apply_key(
            &mut block,
            "pd.invOutWatts",
            &serde_json::json!(225),
        );
        block.source = Source::Mqtt;
        a.apply_update(Update::Snapshot(Box::new(block)));
        let d = &a.devices[0];
        assert_eq!(d.source, Source::Mqtt, "the newest source wins");
        assert_eq!(
            d.power.total_out_w,
            Some(245.0),
            "the pushed block replaces the total"
        );
        assert_eq!(
            d.battery.soc,
            Some(85.0),
            "the charge level comes from the full map"
        );
        assert_eq!(
            d.battery.soh,
            Some(99.0),
            "the health figure survives a partial block"
        );
        // `inv.outputWatts` (272 in the fixture) is the inverter's own measurement and
        // outranks the `pd.invOutWatts` copy, so one measurement is never counted twice.
        assert_eq!(d.power.ac_out_w, Some(272.0));
        assert_eq!(
            d.power.ac_in_w,
            Some(0.0),
            "the pd copy fills the field only when `inv` has not reported"
        );
    }

    #[test]
    fn a_block_without_the_product_type_keeps_the_known_model() {
        // A status-only push carries no product type, so its snapshot is `Unknown`.
        // The known model and everything the model's key table filled must survive.
        let mut a = app();
        let raw = serde_json::from_str::<serde_json::Value>(include_str!(
            "../tests/fixtures/quota_all.json"
        ))
        .expect("fixture");
        let mut full = DeviceSnapshot::new("R351ZEB5PGC90303", "DELTA 2 Max", 81);
        for (k, v) in raw.as_object().unwrap() {
            crate::telemetry::normalizer::apply_key(&mut full, k, v);
        }
        full.source = Source::Rest;
        a.apply_update(Update::Snapshot(Box::new(full)));
        let mut status = DeviceSnapshot::new("R351ZEB5PGC90303", "DELTA 2 Max", 0);
        status.source = Source::Mqtt;
        a.apply_update(Update::Snapshot(Box::new(status)));
        let d = &a.devices[0];
        assert_eq!(d.family, Family::Delta2, "the model is not forgotten");
        assert_eq!(
            d.battery.temp_c,
            Some(29.0),
            "the temperature is still shown"
        );
        assert_eq!(d.battery.soc, Some(85.0));
        assert!((d.battery.capacity_wh.unwrap() - 2119.4).abs() < 1.0);
    }

    #[test]
    fn snapshot_replaces_by_serial() {
        let mut a = app();
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 80.0, 100.0, 50.0))));
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 75.0, 0.0, 300.0))));
        assert_eq!(a.devices.len(), 1);
        assert_eq!(a.devices[0].battery.soc, Some(75.0));
    }

    #[test]
    fn keyboard_navigation_cycles_screens() {
        let mut a = app();
        a.handle_action(Action::NextScreen);
        assert_eq!(a.screen.label(), "Devices");
        a.handle_action(Action::PrevScreen);
        assert_eq!(a.screen.label(), "Dashboard");
        a.handle_action(Action::PrevScreen);
        assert_eq!(a.screen.label(), "Settings", "wraps backwards");
    }

    #[test]
    fn selection_wraps_and_skips_filtered_devices() {
        let mut a = app();
        a.apply_update(Update::DeviceList(vec![
            DiscoveredDevice {
                sn: "AAA".into(),
                product_name: "DELTA 2 Max".into(),
                online: true,
                product_type: Some(81),
            },
            DiscoveredDevice {
                sn: "BBB".into(),
                product_name: "RIVER 2 Pro".into(),
                online: true,
                product_type: Some(72),
            },
        ]));
        a.handle_action(Action::Down);
        assert_eq!(a.selected, 1);
        a.handle_action(Action::Down);
        assert_eq!(a.selected, 0);
        a.set_filter("river");
        a.handle_action(Action::Down);
        assert_eq!(a.selected, 1, "only the filtered device is selectable");
    }

    #[test]
    fn alert_cooldown_suppresses_duplicates() {
        let mut a = app();
        for soc in [15.0, 15.0, 15.0] {
            a.apply_update(Update::Snapshot(Box::new(snap("AAA", soc, 0.0, 100.0))));
        }
        let low = a
            .alerts
            .iter()
            .filter(|al| al.kind == AlertKind::BatteryLow)
            .count();
        assert_eq!(low, 1, "same alert is not repeated inside the cooldown");
    }

    #[test]
    fn controls_are_off_by_default_and_nothing_is_sent() {
        let mut a = app();
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        a.handle_action(Action::Devices);
        a.handle_action(Action::Enter); // details screen + dialog request
        assert!(
            a.pending_control.is_none(),
            "the config has controls off, so the dialog never opens"
        );
        assert!(a.status.contains("controls are off"));
        assert!(
            a.sent_commands().is_empty(),
            "no command reached the services"
        );
    }

    #[test]
    fn a_control_needs_confirmation_before_it_is_sent() {
        let mut a = app();
        a.config.controls.enabled = true;
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        a.handle_action(Action::Devices);
        a.handle_action(Action::Enter); // details screen, dialog opens
        assert!(a.pending_control.is_some());
        assert!(
            a.sent_commands().is_empty(),
            "the first Enter only opens the dialog"
        );
        // The dialog names the serial the user can see and the value.
        let dialog = a.control_dialog().expect("dialog");
        assert!(dialog.contains("charge limit"), "the label is in words");
        assert!(dialog.contains(MASKED), "the serial is masked on screen");
        a.handle_action(Action::Confirm);
        assert!(a.pending_control.is_none());
        assert_eq!(
            a.sent_commands().len(),
            1,
            "the confirmation sends one command"
        );
        assert!(matches!(
            &a.sent_commands()[0],
            Command::Control { sn, control, value }
                if sn == SN && control.id == "charge_limit" && *value == 100
        ));
    }

    #[test]
    fn cancel_never_sends_and_the_value_stays_inside_the_devices_range() {
        let mut a = app();
        a.config.controls.enabled = true;
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        a.handle_action(Action::Devices);
        a.handle_action(Action::Enter);
        a.handle_action(Action::Right); // charge limit 100 -> +5, clamped at 100
        assert_eq!(
            a.pending_control.as_ref().expect("candidate").value,
            100,
            "the top of the range cannot be passed"
        );
        a.handle_action(Action::Left); // 100 -> 95
        assert_eq!(a.pending_control.as_ref().expect("candidate").value, 95);
        a.handle_action(Action::Cancel);
        assert!(a.pending_control.is_none(), "cancel drops the command");
        assert!(a.sent_commands().is_empty());
        // A device that is not reporting is not sent a command: the confirmation
        // refuses, so the user sees a refusal, not a silent failure.
        let mut off = snap(SN, 5.0, 0.0, 100.0);
        off.online = false;
        a.apply_update(Update::Snapshot(Box::new(off)));
        a.handle_action(Action::Right); // 100 -> 95, the dialog is open
        a.handle_action(Action::Confirm);
        assert!(
            a.pending_control.is_some(),
            "the command stays unconfirmed when the device is not reporting"
        );
        assert!(
            a.status.contains("not reporting"),
            "an offline device is never commanded"
        );
        assert!(a.sent_commands().is_empty(), "nothing was sent");
    }

    #[test]
    fn a_device_with_no_verified_commands_is_offered_nothing() {
        let mut a = app();
        a.config.controls.enabled = true;
        // 76 is the PowerStream product type: no verified commands.
        a.apply_update(Update::DeviceList(vec![DiscoveredDevice {
            sn: "STREAM".into(),
            product_name: "Power Stream".into(),
            online: true,
            product_type: Some(76),
        }]));
        a.handle_action(Action::Devices);
        a.handle_action(Action::Enter);
        assert!(a.pending_control.is_none());
        assert!(a.status.contains("no verified controls"));
        assert!(
            a.control_rows().is_empty(),
            "the panel shows nothing to pick"
        );
    }

    #[test]
    fn the_device_answer_is_recorded_as_an_alert() {
        let mut a = app();
        a.apply_update(Update::ControlReply {
            sn: SN.to_string(),
            code: "0".to_string(),
            message: "accepted".to_string(),
        });
        assert!(a.alerts.iter().any(|al| al.kind == AlertKind::CommandReply));
        // The refusal codes are warnings, not successes.
        a.apply_update(Update::ControlReply {
            sn: SN.to_string(),
            code: "-2".to_string(),
            message: "device offline".to_string(),
        });
        assert!(
            a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::CommandReply && al.level == Level::Warn),
            "code -2 is not reported as success"
        );
    }

    #[test]
    fn a_command_and_the_device_answer_are_both_shown() {
        let mut a = app();
        a.config.controls.enabled = true;
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        a.handle_action(Action::Devices);
        a.handle_action(Action::Enter); // dialog opens
        a.handle_action(Action::Confirm); // command sent
        a.apply_update(Update::ControlReply {
            sn: SN.to_string(),
            code: "0".to_string(),
            message: "mock accepted".to_string(),
        });
        assert!(
            a.alerts.iter().any(|al| al.kind == AlertKind::Command),
            "the command the user sent is recorded"
        );
        assert!(
            a.alerts.iter().any(|al| al.kind == AlertKind::CommandReply),
            "the device's answer is recorded as its own alert, so the reply is not \
             swallowed by the send's cooldown"
        );
    }

    #[test]
    fn the_control_list_shows_what_the_device_reported() {
        let mut a = app();
        a.config.controls.enabled = true;
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        let rows = a.control_rows();
        assert!(!rows.is_empty());
        // The live device reports maxChargeSoc 100, so the row says 100 %.
        let charge = rows
            .iter()
            .find(|(label, _, _, _)| label == "charge limit")
            .expect("row");
        assert_eq!(charge.1, "100 %");
        assert!(
            rows.iter().any(|(_, _, _, sel)| *sel),
            "one row is selected"
        );
    }

    #[test]
    fn critical_battery_is_reported_as_critical() {
        let mut a = app();
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 5.0, 0.0, 100.0))));
        assert!(
            a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::BatteryCritical)
        );
        assert!(!a.alerts.iter().any(|al| al.kind == AlertKind::BatteryLow));
    }

    #[test]
    fn fps_key_cycles_the_frame_rate_and_the_tick_length() {
        let mut a = app();
        assert_eq!(a.config.ui.fps, 30);
        let tick = a.config.tick_interval();
        a.handle_action(Action::CycleFps);
        assert_eq!(a.config.ui.fps, 60);
        assert!(a.config.tick_interval() < tick, "60 fps ticks faster");
        assert!(
            a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::Setting && al.text.contains("60 fps"))
        );
        a.handle_action(Action::CycleFps);
        assert_eq!(a.config.ui.fps, 15, "the cycle wraps back to the low end");
        a.handle_action(Action::CycleFps);
        assert_eq!(a.config.ui.fps, 30, "and continues upward from there");
    }

    #[test]
    fn temperature_unit_key_switches_the_unit_and_the_display() {
        let mut a = app();
        a.handle_action(Action::ToggleTempUnit);
        assert_eq!(a.config.ui.temperature_unit, TempUnit::Fahrenheit);
        assert_eq!(
            temp(
                Some(20.0),
                a.config.ui.temperature_unit == TempUnit::Fahrenheit
            ),
            "68 °F"
        );
        a.handle_action(Action::ToggleTempUnit);
        assert_eq!(a.config.ui.temperature_unit, TempUnit::Celsius);
        assert_eq!(
            temp(
                Some(20.0),
                a.config.ui.temperature_unit == TempUnit::Fahrenheit
            ),
            "20 °C"
        );
    }

    #[test]
    fn serial_display_key_is_reported_as_a_change() {
        let mut a = app();
        a.handle_action(Action::ToggleSerial);
        assert!(a.config.ui.show_serial);
        a.handle_action(Action::ToggleSerial);
        assert!(!a.config.ui.show_serial);
    }

    #[test]
    fn high_output_above_the_configured_limit_is_reported() {
        let mut a = app();
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 60.0, 0.0, 2500.0))));
        assert_eq!(
            a.alerts
                .iter()
                .filter(|al| al.kind == AlertKind::HighOutput)
                .count(),
            1
        );
        // The alert names the limit it came from, so the number is not a mystery.
        assert!(a.alerts.iter().any(|al| al.text.contains("2000 W")));
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 60.0, 0.0, 2600.0))));
        assert_eq!(
            a.alerts
                .iter()
                .filter(|al| al.kind == AlertKind::HighOutput)
                .count(),
            1,
            "the cooldown covers the high-output alert too"
        );
        // Under the limit, nothing is reported.
        let mut b = app();
        b.apply_update(Update::Snapshot(Box::new(snap("AAA", 60.0, 0.0, 1900.0))));
        assert!(
            !b.alerts.iter().any(|al| al.kind == AlertKind::HighOutput),
            "the default limit is not crossed"
        );
    }

    #[test]
    fn offline_alerts_can_be_switched_off_without_losing_the_state() {
        let mut a = app();
        a.config.alerts.notify_disconnected = false;
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 50.0, 100.0, 0.0))));
        let mut off = snap("AAA", 50.0, 100.0, 0.0);
        off.online = false;
        a.apply_update(Update::Snapshot(Box::new(off)));
        assert!(
            !a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::DeviceOffline),
            "the user asked not to be told"
        );
        assert!(!a.devices[0].online, "the state is tracked for the UI");
        // With the flag on, coming back online is reported.
        a.config.alerts.notify_disconnected = true;
        a.apply_update(Update::Snapshot(Box::new(snap("AAA", 50.0, 100.0, 0.0))));
        assert!(
            a.alerts.iter().any(|al| al.kind == AlertKind::DeviceOnline),
            "the transition is reported once the notices are on"
        );
    }

    #[test]
    fn offline_transition_alerts_once() {
        let mut a = app();
        let mut online = snap("AAA", 50.0, 0.0, 100.0);
        a.apply_update(Update::Snapshot(Box::new(online.clone())));
        online.online = false;
        a.apply_update(Update::Snapshot(Box::new(online.clone())));
        assert!(
            a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::DeviceOffline)
        );
        online.online = true;
        a.apply_update(Update::Snapshot(Box::new(online)));
        assert!(a.alerts.iter().any(|al| al.kind == AlertKind::DeviceOnline));
    }

    #[test]
    fn transport_transitions_alert() {
        let mut a = app();
        a.apply_update(Update::Transport(
            Transport::Mqtt,
            TransportInfo::new(ConnState::Connected),
        ));
        a.apply_update(Update::Transport(
            Transport::Mqtt,
            TransportInfo::new(ConnState::Reconnecting),
        ));
        assert!(
            a.alerts
                .iter()
                .any(|al| al.kind == AlertKind::TransportDown)
        );
    }

    #[test]
    fn history_samples_are_throttled() {
        let mut a = app();
        a.config.history.sample_interval_seconds = 10;
        for i in 0..100 {
            a.now = Utc::now() + chrono::Duration::seconds(i as i64 * 10);
            a.apply_update(Update::Snapshot(Box::new(snap(
                "AAA",
                50.0 - i as f64,
                100.0,
                50.0,
            ))));
        }
        assert_eq!(a.history_for("AAA").len(), 100);
    }

    #[test]
    fn quit_stops_the_loop() {
        let mut a = app();
        a.handle_action(Action::Quit);
        assert!(!a.running);
    }

    #[test]
    fn scenario_cycles_in_mock_only() {
        let mut a = app();
        a.handle_action(Action::CycleScenario);
        assert_eq!(a.scenario, Scenario::AcCharging);
        let mut live = App::new(Config::default(), false, Scenario::SolarCharging);
        live.handle_action(Action::CycleScenario);
        assert_eq!(
            live.scenario,
            Scenario::SolarCharging,
            "no scenario in live mode"
        );
    }

    #[test]
    fn serial_masking_is_respected() {
        let mut a = app();
        a.apply_update(Update::Snapshot(Box::new(snap(
            "R351ZEB5PGC90303",
            50.0,
            0.0,
            0.0,
        ))));
        a.push_alert(
            AlertKind::BatteryLow,
            Level::Warn,
            "R351ZEB5PGC90303",
            "low",
        );
        let text = a.alerts.back().unwrap().sn.clone();
        assert!(!text.contains("ZEB5PGC90303"));
        a.config.ui.show_serial = true;
        a.push_alert(
            AlertKind::BatteryLow,
            Level::Warn,
            "R351ZEB5PGC90303",
            "low",
        );
        assert_eq!(a.alerts.back().unwrap().sn, "R351ZEB5PGC90303");
    }

    #[test]
    fn freshness_reports_newest_device() {
        let mut a = app();
        let mut s = snap("AAA", 50.0, 0.0, 0.0);
        s.last_update = Utc::now();
        a.apply_update(Update::Snapshot(Box::new(s)));
        let f = a.freshness().unwrap();
        assert!(f < 2.0);
    }

    #[test]
    fn source_label_prefers_mqtt_then_rest() {
        let mut a = app();
        a.mock = false;
        assert_eq!(a.source_label(), "OFFLINE");
        a.apply_update(Update::Transport(
            Transport::Rest,
            TransportInfo::new(ConnState::Connected),
        ));
        assert_eq!(a.source_label(), "REST");
        a.apply_update(Update::Transport(
            Transport::Mqtt,
            TransportInfo::new(ConnState::Connected),
        ));
        assert_eq!(a.source_label(), "MQTT");
    }

    #[test]
    fn a_degraded_transport_with_data_is_still_the_source() {
        // One device refusing to report must not make the label claim there is no data.
        let mut a = app();
        a.mock = false;
        let mut d = snap("AAA", 50.0, 100.0, 20.0);
        d.source = Source::Rest;
        a.apply_update(Update::Snapshot(Box::new(d)));
        a.apply_update(Update::Transport(
            Transport::Rest,
            TransportInfo::new(ConnState::Degraded),
        ));
        assert_eq!(a.source_label(), "REST");
    }

    /// Placeholder serials, the convention of the earlier milestones. History is
    /// keyed by the masked form, so the plain serial never reaches the file.
    const SN: &str = "R351ZEB5PGC90303";
    const MASKED: &str = "R3************03";

    /// A writer thread in a temp file: the app's real storage path, end to end.
    fn storage_app(dir: &std::path::Path) -> (App, std::path::PathBuf) {
        let db = dir.join("history.sqlite");
        let (tx, _rx) = tokio::sync::mpsc::channel::<Update>(16);
        let store = crate::storage::spawn(db.clone(), HistoryConfig::default(), tx).expect("spawn");
        let mut a = app();
        a.set_storage(store);
        (a, db)
    }

    #[test]
    fn samples_are_queued_to_the_history_writer() {
        // The app hands rows to the writer thread; the database is what a later
        // analytics range reads, so the rows have to be there.
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut a, db) = storage_app(dir.path());
        // Sample on every update so a short test produces several rows.
        a.config.history.sample_interval_seconds = 0;
        for i in 0..6 {
            a.tick(Utc::now() + chrono::Duration::seconds(i * 2));
            a.apply_update(Update::Snapshot(Box::new(snap(
                SN,
                50.0 + i as f64,
                100.0,
                20.0,
            ))));
        }
        // The writer is a plain thread; give it a moment to commit the batch.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let conn = rusqlite::Connection::open(&db).expect("reopen");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM samples", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 6, "one row per sample interval");
        let key: String = conn
            .query_row("SELECT sn FROM samples LIMIT 1", [], |r| r.get(0))
            .expect("key");
        assert_eq!(key, MASKED, "history is keyed by the masked serial");
        let plain: i64 = conn
            .query_row("SELECT count(*) FROM samples WHERE sn = ?1", (SN,), |r| {
                r.get(0)
            })
            .expect("plain count");
        assert_eq!(plain, 0, "the plain serial is never stored");
        let soc: f64 = conn
            .query_row("SELECT soc FROM samples ORDER BY t LIMIT 1", [], |r| {
                r.get(0)
            })
            .expect("first row");
        assert_eq!(soc, 50.0, "the first sample is the first reading");
    }

    #[test]
    fn a_registered_device_is_recorded_under_the_masked_serial() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut a, db) = storage_app(dir.path());
        a.apply_update(Update::DeviceList(vec![DiscoveredDevice {
            sn: SN.to_string(),
            product_name: "DELTA 2 Max".to_string(),
            online: true,
            product_type: Some(81),
        }]));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let conn = rusqlite::Connection::open(&db).expect("reopen");
        let model: String = conn
            .query_row("SELECT model FROM devices WHERE sn = ?1", (MASKED,), |r| {
                r.get(0)
            })
            .expect("device row");
        assert_eq!(model, "DELTA 2 Max");
        let plain: i64 = conn
            .query_row("SELECT count(*) FROM devices WHERE sn = ?1", (SN,), |r| {
                r.get(0)
            })
            .expect("plain count");
        assert_eq!(plain, 0, "the plain serial is never stored");
    }

    #[test]
    fn events_are_recorded_under_the_masked_serial() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut a, db) = storage_app(dir.path());
        // Showing the full serial in the UI must not change what the file holds.
        a.config.ui.show_serial = true;
        a.now = Utc::now();
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 5.0, 0.0, 100.0))));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let conn = rusqlite::Connection::open(&db).expect("reopen");
        let key: String = conn
            .query_row(
                "SELECT sn FROM events WHERE kind = 'BatteryCritical' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("event row");
        assert_eq!(key, MASKED, "the event is keyed by the masked serial");
        let plain: i64 = conn
            .query_row("SELECT count(*) FROM events WHERE sn = ?1", (SN,), |r| {
                r.get(0)
            })
            .expect("plain count");
        assert_eq!(plain, 0, "the plain serial is never stored");
        assert_eq!(
            a.alerts
                .iter()
                .find(|al| al.kind == AlertKind::BatteryCritical)
                .expect("alert")
                .sn,
            SN,
            "the UI shows the serial the user asked to see"
        );
    }

    #[test]
    fn database_rows_merge_with_memory_without_duplicates() {
        let mut a = app();
        a.now = Utc::now();
        let recent = a.now - chrono::Duration::seconds(30);
        let older = a.now - chrono::Duration::seconds(3600);
        a.history
            .entry(MASKED.to_string())
            .or_insert_with(|| History::new(120))
            .push(Sample {
                t: recent,
                soc: 50.0,
                in_w: 100.0,
                out_w: 0.0,
                solar_w: 0.0,
                temp_c: 25.0,
                bat_w: -100.0,
            });
        // The database returns the same instant plus one the memory window has lost.
        // The writer answers with the key it stores under.
        a.apply_update(Update::History(
            MASKED.to_string(),
            vec![
                Sample {
                    t: recent,
                    soc: 50.0,
                    in_w: 100.0,
                    out_w: 0.0,
                    solar_w: 0.0,
                    temp_c: 25.0,
                    bat_w: -100.0,
                },
                Sample {
                    t: older,
                    soc: 70.0,
                    in_w: 300.0,
                    out_w: 100.0,
                    solar_w: 200.0,
                    temp_c: 28.0,
                    bat_w: 200.0,
                },
            ],
        ));
        a.range = 3; // 24 hours
        let rows = a.range_samples(SN);
        assert_eq!(rows.len(), 2, "the shared instant is counted once");
        assert_eq!(rows[0].t, older, "oldest first");
        assert_eq!(
            a.db_rows(SN),
            2,
            "the cache is found through the masked key"
        );
    }

    #[test]
    fn a_range_switch_asks_the_database_once_per_range() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (tx, _rx) = tokio::sync::mpsc::channel::<Update>(16);
        let store = crate::storage::spawn(
            dir.path().join("history.sqlite"),
            HistoryConfig::default(),
            tx,
        )
        .expect("spawn");
        let mut a = app();
        a.set_storage(store);
        a.now = Utc::now();
        // Nothing in memory: a 24-hour range can only come from the database.
        a.range = 3;
        a.request_history("SN1");
        let first = a.last_query.expect("the app asks for the range");
        assert_eq!(first.0, 3);
        // Same range, same instant: throttled, so keypresses do not queue reads.
        a.request_history("SN1");
        assert_eq!(
            a.last_query,
            Some(first),
            "the same range is not asked twice"
        );
        a.tick(a.now + chrono::Duration::seconds(1));
        a.request_history("SN1");
        assert_eq!(
            a.last_query,
            Some(first),
            "still inside the throttle window"
        );
        // A different range is asked straight away, and the same range again once
        // the throttle window has passed.
        a.range = 4;
        a.request_history("SN1");
        let second = a.last_query.expect("the new range is asked");
        assert_eq!(second.0, 4);
        a.tick(a.now + chrono::Duration::seconds(QUERY_THROTTLE_SECS));
        a.request_history("SN1");
        assert_eq!(a.last_query.map(|q| q.0), Some(4));
        assert!(
            a.last_query.expect("asked again").1 > second.1,
            "the same range is asked again past the window"
        );
    }

    #[test]
    fn a_saturated_history_queue_is_reported_once() {
        // The writer is behind; the app keeps running and says so a single time.
        let mut a = app();
        let (store, _rx) = crate::storage::pending_handle(2);
        a.set_storage(store);
        a.now = Utc::now();
        a.config.history.sample_interval_seconds = 0;
        for i in 0..10 {
            a.tick(a.now + chrono::Duration::seconds(i));
            // Each report is a different measurement, so every one is a row.
            a.apply_update(Update::Snapshot(Box::new(snap(
                "SN1",
                50.0 + i as f64,
                100.0,
                20.0,
            ))));
        }
        let behind = a
            .alerts
            .iter()
            .filter(|al| al.kind == AlertKind::StorageBehind)
            .count();
        assert_eq!(behind, 1, "a saturated queue is not a stream of alerts");
    }

    /// The mock-mode hint and the plaintext-backend label have to reach the
    /// Alerts screen, which renders `alerts`, not `notices`.
    #[test]
    fn a_credential_state_alert_is_on_the_alerts_list() {
        let mut a = app();
        let n = crate::secrets::mock_hint(true).unwrap();
        a.system_alert(AlertKind::Credential, n.level, n.text);
        let last = a.alerts.back().unwrap();
        assert_eq!(last.kind, AlertKind::Credential);
        assert_eq!(last.sn, "system", "a state alert has no device");
        assert!(last.text.contains("wattdeck setup"), "{}", last.text);
    }

    #[test]
    fn identical_reports_are_stored_once_then_on_the_heartbeat() {
        // MQTT repeats the same properties when a station is doing nothing. One
        // measurement is one row; the last row is stored again on the heartbeat, so
        // history says the station is still answering.
        let mut a = app();
        a.config.history.sample_interval_seconds = 1;
        a.apply_update(Update::DeviceList(vec![DiscoveredDevice {
            sn: SN.into(),
            product_name: "DELTA 2 Max".into(),
            online: true,
            product_type: Some(81),
        }]));
        let t0 = Utc::now();
        for i in 0..12 {
            a.tick(t0 + chrono::Duration::seconds(i));
            a.apply_update(Update::Snapshot(Box::new(snap(SN, 50.0, 236.0, 238.0))));
        }
        assert_eq!(
            a.history_for(SN).len(),
            1,
            "twelve identical reports are one row"
        );
        a.tick(t0 + chrono::Duration::seconds(HEARTBEAT_SECS));
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 50.0, 236.0, 238.0))));
        assert_eq!(
            a.history_for(SN).len(),
            2,
            "the heartbeat stores the row again"
        );
        a.tick(t0 + chrono::Duration::seconds(HEARTBEAT_SECS + 1));
        a.apply_update(Update::Snapshot(Box::new(snap(SN, 51.0, 236.0, 238.0))));
        assert_eq!(
            a.history_for(SN).len(),
            3,
            "a changed measurement is a new row"
        );
    }

    #[test]
    fn a_clouds_refusal_marks_one_device_not_the_connection() {
        // The RIVER mini case: the cloud answered for the account and refused this
        // one device. The transport that answered is healthy, so a MQTT feed that is
        // delivering stays healthy.
        let mut a = app();
        a.now = Utc::now();
        a.apply_update(Update::DeviceList(vec![DiscoveredDevice {
            sn: SN.into(),
            product_name: "RIVER mini Plus".into(),
            online: true,
            product_type: Some(71),
        }]));
        a.apply_update(Update::Transport(
            Transport::Rest,
            TransportInfo::new(ConnState::Connected),
        ));
        a.apply_update(Update::Transport(
            Transport::Mqtt,
            TransportInfo::new(ConnState::Connected),
        ));
        a.apply_update(Update::DeviceRefusal {
            sn: SN.to_string(),
            code: "1006".to_string(),
            message: "device refusal".to_string(),
        });
        let d = a.selected_device().expect("the device is listed");
        assert_eq!(
            d.state(a.now, a.stale_window_secs()),
            DeviceState::Unsupported,
            "the refusal is the device's state, not the connection's"
        );
        assert_eq!(
            d.last_refusal.as_deref(),
            Some("code 1006: device refusal"),
            "the cloud's words are kept verbatim"
        );
        assert_eq!(
            a.connection.rest(),
            ConnState::Connected,
            "one device's refusal does not degrade the transport"
        );
    }

    #[test]
    fn history_is_read_back_from_the_database_after_a_restart() {
        // A restart loses the in-memory ring. The chart draws what the database
        // holds, so a fresh app asks for the range it is showing.
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("history.sqlite");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Update>(16);
        let store =
            crate::storage::spawn(db.clone(), HistoryConfig::default(), tx).expect("writer");
        let mut first = app();
        first.set_storage(store.clone());
        first.config.history.sample_interval_seconds = 0;
        // Hours of history, oldest first, all before the restart.
        let t0 = Utc::now() - chrono::Duration::seconds(10800);
        for i in 0..4 {
            first.tick(t0 + chrono::Duration::seconds(i * 3600));
            first.apply_update(Update::Snapshot(Box::new(snap(
                SN,
                50.0 + i as f64 * 10.0,
                240.0,
                120.0,
            ))));
        }
        // The writer is a plain thread; give it a moment to commit the batch.
        std::thread::sleep(std::time::Duration::from_millis(500));
        // The restart: a new app, the same database, nothing in memory.
        let mut second = app();
        second.set_storage(store);
        second.now = Utc::now();
        second.range = 3; // 24 hours
        assert_eq!(second.range_samples(SN).len(), 0, "the ring buffer is gone");
        second.request_history(SN);
        let answer = rx.blocking_recv().expect("the writer answers the query");
        second.apply_update(answer);
        let rows = second.range_samples(SN);
        assert_eq!(rows.len(), 4, "the rows in the database are the history");
        assert_eq!(
            (rows[3].t - rows[0].t).num_seconds(),
            10800,
            "oldest first, three hours apart"
        );
        assert!(
            second.range_covered(SN),
            "hours of span cover the 24 h range"
        );
    }
}
