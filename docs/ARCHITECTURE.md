# Architecture

## Shape

One process, three kinds of work, joined only by channels:

```
  MQTT broker ──► MqttProvider ─┐
                                ├─► mpsc<Update> (256) ─► App ─► ratatui render
  REST API    ──► LiveProvider ─┘                          ▲
                                                           │
  GitHub    ──► update task ───────────────────────────────┤
                                                           │
  SQLite    ◄── storage writer thread ◄─ mpsc<StorageCmd> ─┘ (64)
```

- `main.rs` owns the terminal, the input stream and the update channel. It runs
  the providers as tokio tasks and the storage writer as a plain thread.
- Providers (`services/live_provider.rs`, `mqtt/client.rs`) talk to the network.
  They never touch the UI.
- `App` (`app.rs`) is the single source of UI truth: device snapshots, alerts,
  history cache, the pending control, and the selected screen. It is pure state
  plus the rules that turn updates into state, and it is the module the tests
  exercise (unit tests, no network, no terminal).
- `ui/` renders `App`. It reads state; it does not change it.
- `secrets/` owns where the keys come from and where they go: one trait, the
  keyring and plaintext-file impls, the probe that picks the backend, the
  `wattdeck setup` wizard behind prompt and device-check traits, and the
  precedence env → keyring → file. It is the only module that touches credentials.
- `update/` (`src/update/mod.rs`, `src/update/install.rs`) is the launch check: one
  GitHub request, the asset verified against `SHA256SUMS.txt` before extraction,
  installed over the binary that is running. It never touches the UI; the result
  comes back as one `Update::Release`, and the footer, the Alerts screen and the
  printed restart line read the state `App` made from it.

The invariant that matters most: **the UI never calls the API and never touches
the database.** A slow network or a slow disk cannot stall a frame. Rendering is
driven by `App`'s state, and the loop draws when something changed
(`main.rs:499-540`), so an idle app draws no frames at 30 FPS.

## Data flow

1. **MQTT** is the primary source. The device pushes partial blocks on
   `/open/{account}/{sn}/quota`, the online flag on `status`, and command
   acknowledgements on `set_reply`. `mqtt/payload.rs` turns each payload into
   dotted property keys; `telemetry/normalizer.rs` merges the partial blocks into
   the device's snapshot, prefixing block keys (`pdStatus` → `pd.*`) so the two
   sources use the same names.
2. **REST** is the backfill: `GET /iot-open/sign/device/quota/all?sn=…` returns
   the whole property map, so a device that has never pushed has a complete
   snapshot, and a missed partial block is repaired on the next poll.
3. `telemetry/models.rs` is the normalized `DeviceSnapshot`: battery, power,
   temperatures, cells, plus `raw` — every reported key/value pair, kept
   verbatim. Unknown properties are preserved and shown in the raw inspector.
4. The app samples the snapshot on the history cadence, sends it to the storage
   thread, and keeps a small in-memory window per device for the sparkline.
   Range requests go to the database; results come back as updates and are
   cached by masked serial, so the analytics screen redraws from memory.

## Bounded everything

- `mpsc<Update>` 256, `mpsc<Command>` 8, storage queue 64.
- Storage sends use `try_send`: a queue that is full is reported once as
  `AlertKind::StorageBehind`, not retried, not allowed to block the app.
- History requests are throttled (`QUERY_THROTTLE_SECS = 5`), so scrubbing the
  analytics range does not hammer the database.
- The writer commits in batches of 64 commands, in WAL mode with
  `synchronous = NORMAL` and `busy_timeout = 2000`, and purges old rows every
  500 batches.
- Alert cooldown (default 600 s per device+kind) keeps a disconnect from
  producing an alert every poll. Command replies use their own kind so the
  reply is not swallowed by the send's cooldown.

## Secrets

Serials are masked at the boundary that matters: `App::store_key(sn)` is
`mask_serial(sn)`, so every storage command, every history cache key and every
history row uses the masked serial. The history cache is bounded (4096 entries).
Logs mask the serial in published payloads, and `utils/formatting.rs` masks
credential values in the log file.

## Controls

`controls/mod.rs` is the catalog: module type, operate type, parameter, value
range, the property key the device reports the setting under, the capability the
device must report, and the extra parameters the verified command carries. The
extra parameters are echoed from the device's own reported values (the "as is"
sentinels `-1`/`255` when the device did not report them), so a command changes
one setting and leaves the rest. `mqtt/client.rs` publishes on the set topic and
matches the `set_reply` to the command id the device echoes, so the alert names
the control. The mock provider (`services/telemetry_service.rs`) implements the
same accept path, so the UI, the confirm dialog and the reply path are exercised
in mock mode without a device.

## Why no async runtime in the UI thread

The terminal is driven by a `crossterm` event stream and the update channel in
one `tokio::select!` loop. Network and storage live in their own tasks/threads.
That keeps the frame path short: receive → apply → draw. Nothing on the frame
path can block.
