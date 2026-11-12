use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;

use wattdeck::api::client::Client;
use wattdeck::app::App;
use wattdeck::config;
use wattdeck::input::resolve;
use wattdeck::mqtt::client::MqttProvider;
use wattdeck::services::live_provider::LiveProvider;
use wattdeck::services::telemetry_service::{Scenario, run_mock};
use wattdeck::services::{Command, Notification, Update};
use wattdeck::storage;
use wattdeck::ui;
use wattdeck::utils::formatting::{mask_secrets, mask_values};

#[derive(Parser)]
#[command(
    name = "wattdeck",
    version,
    about = "EcoFlow power station dashboard for the terminal",
    author
)]
struct Cli {
    /// Write the starter config, then exit. The credential template is written
    /// only when no secret service is present.
    #[arg(long)]
    init: bool,
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Verbose logging to stderr, plus a log file in the data directory.
    #[arg(long)]
    debug: bool,
    /// Run against the mock provider. No credentials, no network.
    #[arg(long)]
    mock: bool,
    /// Use the live REST API. Needs keys: the keyring, credentials.toml, or the
    /// ECOFLOW_ACCESS_KEY / ECOFLOW_SECRET_KEY env vars.
    #[arg(long)]
    live: bool,
    /// Start with only this device shown.
    #[arg(long, value_name = "SERIAL")]
    device: Option<String>,
    /// Force read-only mode: no device commands are ever sent.
    #[arg(long)]
    read_only: bool,
    /// Start in a named mock scenario (solar charging, AC charging, high load discharging, fully charged, critical battery, device offline).
    #[arg(long, value_name = "SCENARIO")]
    scenario: Option<String>,
    /// Skip the launch update check: no GitHub request, no download, nothing installed.
    #[arg(long)]
    no_update: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the device list.
    Devices,
    /// Print a one-shot status summary.
    Status,
    /// Export the current telemetry.
    Export {
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Ask for the keys, store them, and show what the keys can see.
    Setup,
    /// Check for a new build, verify its SHA256, install it, print what happened.
    Update,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.debug, cli.cmd.is_some() || cli.init);
    let store = wattdeck::secrets::open()?;
    let file = wattdeck::secrets::file_store::FileStore::default_store()?;
    if let Some(line) = wattdeck::secrets::report_alongside(store.backend(), &file) {
        // A plaintext file the keyring backend does not read is reported once,
        // and touched never.
        tracing::warn!("{line}");
    }

    if cli.init {
        return run_init(store.as_ref());
    }

    let mut cfg = config::load_config(cli.config.as_deref())?;
    if cli.no_update {
        cfg.update.enabled = false;
    }
    if let Ok(exe) = std::env::current_exe() {
        // The leftover from the last Windows update, deleted before the terminal
        // is set up. A no-op on unix when there is no `.old`.
        wattdeck::update::install::cleanup_stale(&exe);
    }
    if cli.read_only {
        cfg.controls.enabled = false;
    }
    let env = wattdeck::secrets::ProcessEnv;
    let creds = wattdeck::secrets::load_with(store.as_ref(), &env)?;
    // The keys are registered so the debug log file masks them. Registration is
    // global, so the log writer sees them from the first line it writes.
    if let Some(c) = creds.as_ref() {
        wattdeck::utils::formatting::register_secret(&c.access_key, "key");
        wattdeck::utils::formatting::register_secret(&c.secret_key, "<redacted>");
    }

    if cli.live && cli.mock {
        anyhow::bail!("--live and --mock cannot be combined; --mock uses no network");
    }
    // Live mode is opt-in: it contacts the API. Without credentials the app runs on
    // the mock provider, and the UI shows a MOCK badge so a mock value is never
    // mistaken for a live reading.
    if cli.live && creds.is_none() {
        anyhow::bail!("live mode needs keys — run: wattdeck setup");
    }
    let use_mock = cli.mock || creds.is_none();
    if creds.is_none() && !cli.mock {
        tracing::warn!("no credentials configured; running in mock mode");
    }
    let scenario = cli
        .scenario
        .as_deref()
        .map(Scenario::from_name)
        .unwrap_or(Some(Scenario::SolarCharging))
        .unwrap_or(Scenario::SolarCharging);

    let source = wattdeck::secrets::source(store.as_ref(), &env);
    match cli.cmd {
        Some(Cmd::Devices) => print_devices(cfg, creds, use_mock, scenario),
        Some(Cmd::Status) => print_status(cfg, creds, use_mock, scenario, &source),
        Some(Cmd::Export { format }) => export(cfg, creds, use_mock, scenario, &format),
        Some(Cmd::Setup) => {
            let mut prompt = wattdeck::secrets::wizard::TerminalPrompt;
            let check = wattdeck::secrets::wizard::LiveCheck::new(&cfg);
            wattdeck::secrets::wizard::run(store.as_ref(), &mut prompt, &check)?;
            Ok(())
        }
        Some(Cmd::Update) => {
            let runtime = tokio::runtime::Runtime::new()?;
            let status = runtime.block_on(wattdeck::update::run(
                &cfg.update,
                env!("CARGO_PKG_VERSION"),
                &config::data_dir()?.join("update"),
                &std::env::current_exe()?,
                None,
            ));
            match status {
                Ok(wattdeck::update::Status::UpToDate) => {
                    println!("wattdeck {} is the latest build", env!("CARGO_PKG_VERSION"));
                }
                Ok(wattdeck::update::Status::Applied { from, to }) => {
                    println!("updated wattdeck {from} → {to} · restart to apply the new build");
                }
                Ok(wattdeck::update::Status::Available { to }) => {
                    println!("wattdeck {to} is newer; nothing was installed");
                }
                // A failure is a named refusal and a non-zero exit, not a silent
                // success: the message names the asset and the sums file.
                Ok(wattdeck::update::Status::Failed(reason)) => {
                    anyhow::bail!("update check failed: {reason}");
                }
                Err(e) => {
                    anyhow::bail!("update check failed: {e:#}");
                }
            }
            Ok(())
        }
        None => {
            let check = wattdeck::update::launch_check(
                &cfg.update,
                cli.no_update,
                use_mock,
                &wattdeck::update::seams(),
            );
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(run_tui(
                cfg, creds, use_mock, scenario, cli.device, &source, check,
            ))
        }
    }
}

/// `--init` writes the starter config. The credential template is written only
/// when the store is the plaintext file, because that is the scripted path; on a
/// machine with a secret service the wizard is the way in. It never prints a key.
fn run_init(store: &dyn wattdeck::secrets::SecretStore) -> Result<()> {
    let dir = config::config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let cfg_path = dir.join("config.toml");
    if cfg_path.exists() {
        println!("config already exists at {}", cfg_path.display());
    } else {
        std::fs::write(&cfg_path, starter_config())?;
        println!("wrote {}", cfg_path.display());
    }
    if store.backend() == "plaintext" {
        let cred_path = dir.join("credentials.toml");
        if !cred_path.exists() {
            std::fs::write(
                &cred_path,
                "[credentials]\naccess_key = \"\"\nsecret_key = \"\"\n",
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600))?;
            }
            println!(
                "wrote {} ({})",
                cred_path.display(),
                wattdeck::secrets::file_store::protection_text()
            );
            println!("fill in your Access Key and Secret Key, then run: wattdeck");
        }
    }
    println!("run: wattdeck setup");
    Ok(())
}

fn starter_config() -> String {
    r#"# wattdeck configuration. Keys live in the OS keyring, or in credentials.toml
# when no secret service is present. Run: wattdeck setup
[api]
region = "EU"
refresh_interval_seconds = 30
timeout_seconds = 10

[mqtt]
enabled = true
reconnect = true
keepalive_secs = 30
refresh_interval_seconds = 30
verify_broker_cert = true

[ui]
theme = "Dark"
color_mode = "Auto"
fps = 30
show_serial = false
temperature_unit = "Celsius"
power_unit = "Auto"

[history]
enabled = true
retention_days = 90
sample_interval_seconds = 5
memory_samples = 120

[analytics]
# Your own electricity price, currency per kWh. 0.0 shows no cost line.
tariff_per_kwh = 0.0

[alerts]
battery_low_percent = 20
battery_critical_percent = 10
high_temperature_c = 45.0
high_output_w = 2000.0
notify_disconnected = true
cooldown_seconds = 600

[update]
# The launch check: one GitHub API request, verify the asset's SHA256, install.
# Off means no network: enabled = false, or wattdeck --no-update
enabled = true
repo = "olexanderboychuk/wattdeck"
timeout_seconds = 10

[controls]
enabled = false
"#
    .to_string()
}

async fn collect_headless(
    cfg: &wattdeck::config::Config,
    creds: Option<wattdeck::config::credentials::Credentials>,
    use_mock: bool,
    scenario: Scenario,
    ticks: usize,
) -> App {
    // `--mock` is the rule, not the credential store: with keys in the keyring, a
    // mock run still uses the mock provider and writes no history, exactly as the
    // TUI does (`run_tui` gates the provider and the storage thread on `use_mock`).
    let live = creds.is_some() && !use_mock;
    let mut app = App::new(cfg.clone(), !live, scenario);
    let (tx, mut rx) = mpsc::channel::<Update>(256);
    let (_cmd_tx, cmd_rx) = mpsc::channel::<Command>(8);
    // Headless runs write the same history the TUI writes, so `export` has real
    // samples. Mock runs never write history, as in the TUI. `storage::open`
    // creates the data directory. Losing history is not worth failing a run, so a
    // data directory that cannot be resolved just means no storage thread.
    if cfg.history.enabled
        && live
        && let Ok(dir) = config::data_dir()
    {
        match storage::spawn(dir.join("history.sqlite"), cfg.history.clone(), tx.clone()) {
            Ok(handle) => app.set_storage(handle),
            Err(e) => {
                tracing::warn!("history database unavailable: {e}");
            }
        }
    }
    let handle = if live {
        // Headless probes use the live provider, so `devices`, `status` and `export`
        // show the same data the TUI shows.
        let provider = LiveProvider::new(cfg, creds.as_ref().unwrap());
        tokio::spawn(provider.run(tx.clone(), cmd_rx))
    } else {
        tokio::spawn(run_mock(scenario, tx.clone(), cmd_rx, 0.1))
    };
    let mqtt_handle = if live && cfg.mqtt.enabled {
        Some(tokio::spawn(
            MqttProvider::new(
                cfg.mqtt.clone(),
                Client::new(&cfg.api, creds.as_ref().unwrap()),
            )
            .run(tx, mpsc::channel::<Command>(1).1),
        ))
    } else {
        None
    };
    let step = if live {
        Duration::from_millis(1000)
    } else {
        Duration::from_millis(100)
    };
    for _ in 0..ticks {
        while let Ok(u) = rx.try_recv() {
            app.apply_update(u);
        }
        app.tick(Utc::now());
        tokio::time::sleep(step).await;
    }
    handle.abort();
    if let Some(h) = mqtt_handle {
        h.abort();
    }
    app
}

fn print_devices(
    cfg: wattdeck::config::Config,
    creds: Option<wattdeck::config::credentials::Credentials>,
    use_mock: bool,
    scenario: Scenario,
) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let app = runtime.block_on(collect_headless(&cfg, creds, use_mock, scenario, 15));
    println!("source: {}", app.source_label());
    println!(
        "{:<14} {:<14} {:>6} {:>8} {:>8} {:>8}",
        "SERIAL", "MODEL", "STATE", "SOC", "IN W", "OUT W"
    );
    for d in &app.devices {
        println!(
            "{:<14} {:<14} {:>6} {:>8} {:>8} {:>8}",
            d.masked_serial(),
            d.product_name,
            if d.online { "online" } else { "offline" },
            d.battery
                .soc
                .map(|s| format!("{:.0}%", s))
                .unwrap_or_else(|| "-".into()),
            d.power
                .total_in_w
                .map(|w| format!("{w:.0}"))
                .unwrap_or_else(|| "-".into()),
            d.power
                .total_out_w
                .map(|w| format!("{w:.0}"))
                .unwrap_or_else(|| "-".into())
        );
    }
    if use_mock {
        eprintln!("note: mock provider; no device was contacted");
    }
    Ok(())
}

fn print_status(
    cfg: wattdeck::config::Config,
    creds: Option<wattdeck::config::credentials::Credentials>,
    use_mock: bool,
    scenario: Scenario,
    source: &str,
) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let app = runtime.block_on(collect_headless(&cfg, creds, use_mock, scenario, 15));
    println!("source: {}", app.source_label());
    println!("credentials: {source}");
    println!("devices: {}", app.devices.len());
    println!("mock: {}", app.mock);
    println!("scenario: {}", app.scenario.label());
    println!("rest: {}", app.connection.rest().label());
    println!("alerts: {}", app.alert_count());
    for d in &app.devices {
        println!(
            "{} {} soc {}",
            d.masked_serial(),
            d.product_name,
            d.battery
                .soc
                .map(|s| format!("{:.0}%", s))
                .unwrap_or_else(|| "-".into())
        );
    }
    // The alert list is the user-visible record of what went wrong, so a one-shot
    // status run prints it too. Serials in alert text are masked by the producer.
    for a in app.alerts.iter().rev().take(8) {
        println!("alert {:?} {} {}", a.level, a.sn, a.text);
    }
    for n in app.notices.iter().rev().take(8) {
        println!("notice {:?} {}", n.level, n.text);
    }
    if use_mock {
        eprintln!("note: mock provider; no device was contacted");
    }
    Ok(())
}

fn export(
    cfg: wattdeck::config::Config,
    creds: Option<wattdeck::config::credentials::Credentials>,
    use_mock: bool,
    scenario: Scenario,
    format: &str,
) -> Result<()> {
    if format != "json" {
        anyhow::bail!("format {format} is not supported; supported format: json");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let app = runtime.block_on(collect_headless(&cfg, creds, use_mock, scenario, 15));
    let devices: Vec<serde_json::Value> = app
        .devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "serial": if cfg.ui.show_serial { d.sn.clone() } else { d.masked_serial() },
                "model": d.product_name,
                "product_type": d.product_type,
                "family": d.family.label(),
                "online": d.online,
                "source": d.source.label(),
                // Every export states where the value came from: a mock value is
                // never presented as a live reading.
                "mock": app.mock,
                "soc_percent": d.battery.soc,
                "remaining_wh": d.battery.remaining_wh,
                "capacity_wh": d.battery.capacity_wh,
                "soh_percent": d.battery.soh,
                "battery_temp_c": d.battery.temp_c,
                "cycles": d.battery.cycles,
                "input_w": d.power.total_in_w,
                "ac_input_w": d.power.ac_in_w,
                "solar_input_w": d.power.solar_in_w,
                "output_w": d.power.total_out_w,
                "ac_output_w": d.power.ac_out_w,
                "usb_output_w": d.power.usb_out_w,
                "last_update": d.last_update.to_rfc3339(),
                // The full property map the device reported, including keys the app
                // does not understand. Serial values are masked.
                "properties": mask_values(&d.raw, &d.sn),
            })
        })
        .collect();
    let out = serde_json::json!({ "devices": devices, "generated_at": Utc::now().to_rfc3339() });
    if app.mock {
        eprintln!("note: mock provider; every value below comes from the mock, not from a device");
    }
    let text = serde_json::to_string_pretty(&out)?;
    io::stdout().write_all(text.as_bytes())?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}

async fn run_tui(
    cfg: wattdeck::config::Config,
    creds: Option<wattdeck::config::credentials::Credentials>,
    use_mock: bool,
    scenario: Scenario,
    device: Option<String>,
    source: &str,
    // Runs the launch update check as a background task.
    check_update: bool,
) -> Result<()> {
    let mut app = App::new(cfg.clone(), use_mock, scenario);
    // The header carries no credential text; the Alerts screen is where state the
    // app wants seen lands: the plaintext-backend label, and the mock-mode hint.
    for n in [
        wattdeck::secrets::backend_alert(source),
        wattdeck::secrets::mock_hint(use_mock),
    ]
    .into_iter()
    .flatten()
    {
        app.system_alert(wattdeck::app::AlertKind::Credential, n.level, n.text);
    }
    if use_mock {
        // The mock device publishes every second, so sampling every second gives a
        // chart that fills up in a short demo run. Live devices throttle samples to
        // the configured interval.
        app.config.history.sample_interval_seconds = 1;
    }
    if let Some(serial) = device {
        app.set_filter(serial);
    }

    let (tx, mut rx) = mpsc::channel::<Update>(64);
    // The update check reports through the same channel the providers use.
    let tx_update = tx.clone();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(8);
    app.set_command_sender(cmd_tx);
    // The loop has to hear about an fps change made from the keyboard.
    let (tick_tx, mut tick_rx) = mpsc::channel::<std::time::Duration>(1);
    app.set_tick_notifier(tick_tx);

    // History goes to SQLite on a writer thread; the app never opens the database
    // itself, so a slow disk cannot stall the UI. A mock run is a demo, so it is
    // never written to the file the real devices' history lives in.
    if cfg.history.enabled && !use_mock {
        match storage::spawn(
            config::data_dir()?.join("history.sqlite"),
            cfg.history.clone(),
            tx.clone(),
        ) {
            Ok(handle) => app.set_storage(handle),
            Err(e) => {
                tracing::warn!("history database unavailable: {e}");
                app.apply_update(Update::Notification(Notification::warn(format!(
                    "history database disabled: {e}"
                ))));
            }
        }
    }

    if use_mock {
        tokio::spawn(run_mock(scenario, tx, cmd_rx, 1.0));
    } else {
        // Live mode: MQTT is the primary source (device pushes), REST polling is the
        // fallback and the backfill. Both run, each with its own connection state,
        // and the app merges their property keys into one snapshot per device.
        let creds = creds.as_ref().expect("credentials checked in main");
        let provider = LiveProvider::new(&cfg, creds);
        // Commands go to the MQTT provider: the set path is verified there, and the
        // REST provider reports it cannot send one.
        tokio::spawn(provider.run(tx.clone(), mpsc::channel::<Command>(1).1));
        if cfg.mqtt.enabled {
            let mqtt = MqttProvider::new(cfg.mqtt.clone(), Client::new(&cfg.api, creds));
            tokio::spawn(mqtt.run(tx, cmd_rx));
        }
    }

    if check_update {
        // The check is a background task: the terminal opens without waiting for
        // GitHub, and the answer arrives as one Update.
        let update_cfg = cfg.update.clone();
        let cache = config::data_dir()?.join("update");
        let exe = std::env::current_exe()?;
        tokio::spawn(async move {
            let status = match wattdeck::update::run(
                &update_cfg,
                env!("CARGO_PKG_VERSION"),
                &cache,
                &exe,
                Some(&tx_update),
            )
            .await
            {
                Ok(s) => s,
                Err(e) => wattdeck::update::Status::Failed(format!("{e:#}")),
            };
            let _ = tx_update.send(Update::Release(status)).await;
        });
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;

    // A panic must not leave the terminal in alt screen. The hook stays installed:
    // the process exits right after, so there is nothing to restore.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    let result = run_loop(&mut terminal, &mut app, &mut rx, &mut tick_rx).await;

    restore_terminal()?;
    // The line is printed after the terminal is restored, so it lands in the
    // scrollback the user reads after quitting.
    if let Some(wattdeck::update::Status::Applied { from, to }) = &app.release {
        println!("updated wattdeck {from} → {to} · restart to apply the new build");
    }
    result
}

fn restore_terminal() -> Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    stdout.flush()?;
    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    rx: &mut mpsc::Receiver<Update>,
    tick_rx: &mut mpsc::Receiver<std::time::Duration>,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(app.config.tick_interval());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;

    while app.running {
        tokio::select! {
            ev = events.next() => {
                match ev {
                    Some(Ok(Event::Key(key))) => {
                        if key.kind == KeyEventKind::Press
                            && let Some(action) = resolve(&app.keys, key) {
                                app.handle_action(action);
                                dirty = true;
                            }
                    }
                    Some(Ok(Event::Resize(_, _))) => dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    // The event stream ended; the terminal is gone, so stop cleanly.
                    None => break,
                }
            }
            u = rx.recv() => {
                if let Some(u) = u {
                    app.apply_update(u);
                    dirty = true;
                }
            }
            d = tick_rx.recv() => {
                // An fps key asked for a new tick length; rebuild the interval.
                if let Some(d) = d {
                    tick = tokio::time::interval(d);
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                }
            }
            _ = tick.tick() => {
                app.tick(Utc::now());
                // A staleness crossing changes the header, so repaint on the tick.
                dirty = true;
            }
        }
        if dirty {
            terminal.draw(|frame| ui::render(app, frame))?;
            dirty = false;
        }
    }
    Ok(())
}

fn init_tracing(debug: bool, console: bool) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(if debug {
            tracing::level_filters::LevelFilter::DEBUG.into()
        } else {
            tracing::level_filters::LevelFilter::INFO.into()
        })
        .from_env_lossy();
    // A log file in the data directory, masked through the values the app has
    // registered as secrets. The worker guard is deliberately leaked so the writer
    // thread lives for the whole run; the process is short-lived. The file is
    // written whatever the level, so the diagnostics the app shows have a record
    // behind them.
    let file_layer = config::data_dir().ok().map(|dir| {
        let appender = tracing_appender::rolling::never(dir, format!("{}.log", config::APP_DIR));
        let (writer, guard) = tracing_appender::non_blocking(appender);
        std::mem::forget(guard);
        // `MakeWriter` for a closure: each line gets its own masked writer, so the
        // masking is applied to every line the layer formats.
        let make = move || Masked::new(writer.clone());
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_writer(make)
    });
    let base = tracing_subscriber::registry().with(filter).with(file_layer);
    // The TUI owns the terminal. A log line on stderr paints over the screen, so
    // console logging is for the headless and wizard paths only.
    if console {
        base.with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .init();
    } else {
        base.init();
    }
}

/// A writer that masks the registered secret values in every line.
struct Masked<B>(B);

impl<B> Masked<B> {
    fn new(inner: B) -> Self {
        Masked(inner)
    }
}

impl<B: std::io::Write> std::io::Write for Masked<B> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf).to_string();
        self.0.write_all(mask_secrets(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
