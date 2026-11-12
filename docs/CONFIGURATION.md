# Configuration

Two files, in the XDG locations (falling back to `$HOME`):

- `~/.config/wattdeck/config.toml` — all settings
- the OS keyring, or `~/.config/wattdeck/credentials.toml` — the two API keys
- `~/.local/share/wattdeck/history.sqlite` — history, plus the log file

`wattdeck setup` is the way in. `wattdeck --init` writes a starter `config.toml`
for a scripted install, and the credential template only when no secret service
is present. A missing `config.toml` is fine: defaults apply, so `wattdeck --mock`
works on a fresh machine. `--config PATH` points at another file.

## Credentials

Precedence: `ECOFLOW_ACCESS_KEY` / `ECOFLOW_SECRET_KEY` → the OS keyring → the
file. `wattdeck status` prints which one this run used.

**Keyring** (preferred): two entries, service `wattdeck`, usernames `access_key`
and `secret_key`, in the freedesktop secret service. `wattdeck setup` writes them
and reads them back; a roundtrip that does not match is reported as a failure.
No prompt is needed to run the app. A locked wallet is an error, not a fallback:
the message names the wallet and says to unlock it and re-run.

**File** (fallback, when no secret service answers):
`~/.config/wattdeck/credentials.toml` at mode 600.

```toml
[credentials]
access_key = "…"
secret_key = "…"
```

Writing it through the wizard needs explicit consent. The app warns (never
printing the contents) when the file is group/world readable, the TUI pushes one
alert naming the plaintext state, `wattdeck status` labels it, and the startup
reports a plaintext file that the keyring backend does not read. `wattdeck setup`
names the plaintext file it did not write — no migration, no silent import.

Keys are never in `config.toml`; the starter file says where the keys live and
names the wizard. `Credentials` has a hand-written `Debug` impl that redacts the
secret key, so keys cannot appear in a panic message or a log line, and the log
file has credential values masked.

## `[api]`

| Key | Default | Meaning |
| --- | --- | --- |
| `region` | `"EU"` | Host `api-e.ecoflow.com`. EU is the region verified for this account; `base_url_override` is the escape hatch |
| `refresh_interval_seconds` | `30` | REST polling cadence, used for the first sync and as the MQTT fallback. **Must be ≥ 5** (rate limits) |
| `base_url_override` | none | Point at a different host |
| `timeout_seconds` | `10` | Request timeout |

## `[mqtt]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | MQTT is the primary telemetry source |
| `reconnect` | `true` | Reconnect inside the poll loop; a 5 s pause prevents a failing broker from spinning |
| `keepalive_secs` | `30` | Clamped to 15–60, the range implementations use |
| `refresh_interval_seconds` | `30` | How often to publish `latestQuotas` — devices slow their pushes when nobody polls. **Must be ≥ 10** |
| `client_id` | none, derived from the account | The broker tolerates about 10 unique ids per day per key, so the id is fixed and stable across runs. Set it explicitly if you want |
| `host_override` | none | For a local broker |
| `tls_override` | none, from `protocol` | `false` is for a local test broker, never for the real one |
| `verify_broker_cert` | `true` | TLS verification |

## `[ui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `"Dark"` | Colour theme |
| `color_mode` | `"Auto"` | `Auto`, `Ansi`, `Rgb` |
| `fps` | `30` | **1–60.** `f` cycles 30 → 60 → 15 |
| `show_serial` | `false` | Serials are masked (`R3************44`); `i` switches |
| `temperature_unit` | `"Celsius"` | `u` toggles °C/°F |
| `power_unit` | `"Auto"` | `Auto` renders 1.4 kW above 1 kW, `Watts` renders 1420 W |
| `show_mock_badge` | `true` | Mock runs carry a `MOCK` badge. Keep this on |

## `[history]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | SQLite history. Mock runs never write history, whatever this says |
| `retention_days` | `90` | **≥ 1.** Purged on the writer's cadence |
| `sample_interval_seconds` | `5` | **≥ 1.** One sample per device. A report with the same numbers as the last is not stored twice; the last identical sample is stored again every 10 minutes |
| `memory_samples` | `120` | **≥ 8.** In-memory sparkline window per device |

## `[analytics]`

| Key | Default | Meaning |
| --- | --- | --- |
| `tariff_per_kwh` | `0.0` | Your electricity price, currency per kWh. Cost lines appear only when it is set; `0.0` shows none |

Ranges shown: per minute, 5 minutes, hourly, daily, weekly, newest first.
Gaps longer than 900 s are reported, so a bucket is never presented as
continuous when the device was not reporting.

## `[alerts]`

| Key | Default | Meaning |
| --- | --- | --- |
| `battery_low_percent` | `20` | Warning level |
| `battery_critical_percent` | `10` | **Must not exceed `battery_low_percent`** |
| `high_temperature_c` | `45.0` | Warning level |
| `high_output_w` | `2000.0` | Alert when a device's measured total output reaches this |
| `notify_disconnected` | `true` | Set to `false` to suppress offline/online notices |
| `cooldown_seconds` | `600` | Per device + alert kind, so a disconnect is not re-alerted every poll. Command replies use their own kind, so a reply is not swallowed by the send's cooldown |

## `[controls]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | **Controls are off by default.** Enabling them is a config decision. `--read-only` forces them off for a run, whatever this says |

Enabling controls does not send anything: a command needs a pick (`c` /
`shift+c`), a value (`left` / `right`), and a confirmation (`enter` / `y`;
`n` / `esc` cancels and sends nothing). The command is refused if the device is
offline, if the value is outside the range, or if the device has reported no
settings. See [device support](DEVICE_SUPPORT.md) for the verified command list.

## `[update]`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Check for a new build on every launch, verify the asset's SHA256, install it, and say `restart to apply`. `false`, or `wattdeck --no-update`, means no network. A mock run never checks, whatever this says |
| `repo` | `"olexanderboychuk/wattdeck"` | The fork escape hatch: the repo the latest-release query is aimed at |
| `timeout_seconds` | `10` | **1–60.** GitHub request timeout |

`wattdeck update` runs the same check without the TUI: check, verify, install,
print what happened. The check is one unauthenticated GitHub API request per
launch; GitHub's documented limit is 60 requests per hour per IP.

## `[keys]`

Remapping: `action_name = "key"`, e.g. `quit = "ctrl+q"`. Actions are the names
in `src/input/keybindings.rs` (`dashboard`, `devices`, `analytics`, `alerts`,
`settings`, `next_screen`, `prev_screen`, `up`, `down`, `left`, `right`,
`next_control`, `prev_control`, `confirm`, `cancel`, `refresh`,
`toggle_serial`, `toggle_temp_unit`, `cycle_fps`, `quit`, `cycle_scenario`,
`page_up`, `page_down`, `home`, `end`, `enter`, `escape`). Navigation keys are
read-only: no navigation key sends a command.

## Validation

`Config::validate()` runs on load and refuses: `fps` outside 1–60,
`api.refresh_interval_seconds` < 5, `history.sample_interval_seconds` < 1,
`retention_days` < 1, `memory_samples` < 8, negative `tariff_per_kwh`,
`battery_critical_percent` > `battery_low_percent`,
`mqtt.refresh_interval_seconds` < 10, `update.timeout_seconds` outside 1–60, and
an empty `update.repo`. The app refuses to start on a config that would poll
faster than the API tolerates.
