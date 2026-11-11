# Development

## Build and test

`make` is the normal entry point. Plain `make` is the full verify: build, the
unit tests, `fmt --check`, `clippy --all-targets`. `make help` lists the
targets.

```
make            # full verify
make release    # cargo build --release
make test       # the unit tests, plus the CLI-surface test
make fmt        # rustfmt · make fmt-check
make clippy     # --all-targets, in a separate target dir
make run        # release binary, mock mode
make run-live   # release binary, real devices
make install    # into $(PREFIX)/bin, default ~/.local
make clean
```

The Makefile pins one toolchain: `RUST` names the standalone Rust install,
`CARGO` is its cargo, and its `bin` dir is first in `PATH`, so cargo, rustc,
`clippy-driver` and `rustfmt` are all the same version. `LD_LIBRARY_PATH` points
at that toolchain's `lib` dir, which `rustfmt` and `clippy-driver` need for
`librustc_driver`. The system rust is a newer cargo and rustc with no
`clippy-driver` at all, so mixing them makes clippy reject every dependency with
`E0514`. Clippy uses a separate target dir so a clippy run does not invalidate
`./target`. All of it is overridable: `make clippy RUST=… LD_LIBRARY_PATH=…
CLIPPY_TARGET=…`.

The tests are unit tests in the modules: no network, no terminal, no device.
`tests/cli.rs` runs the binary and asserts the CLI name, which touches no
network, no credentials and no device. The live path is verified by probes,
described below. Cargo works directly, as the
targets are cargo commands:

```
cargo build --release && cargo test --lib
LD_LIBRARY_PATH=~/.local/rust-1.99.0/lib cargo fmt --check
CARGO_TARGET_DIR=/tmp/opencode/clippy-target LD_LIBRARY_PATH=~/.local/rust-1.99.0/lib cargo clippy --all-targets
```

## Mock mode

`--mock` runs with no credentials and no network. Scenarios, named by the label
the CLI matches (`--scenario NAME`, `s` cycles them): `solar charging`,
`AC charging`, `high load discharging`, `fully charged`, `critical battery`,
`device offline`. The mock provider
(`services/telemetry_service.rs`) emits the DELTA 2 dialect, including the
settings the controls write, so the control flow, the dialog, the reported rows
and the replies are all exercised offline. `MockSim::accept(sn, control,
value)` is the single accept path the tests use.

Mock runs never write history, mock serials are placeholders, and every mock
screen shows the `MOCK` badge.

## The terminal, without a terminal

`script` is not installed, so PTY verification uses
`/tmp/opencode/keyprobe.py` (spawn the binary in a pty, send keys, reconstruct
the screen after each key; it prints the text the terminal drew, colours
dropped). The file lives in a temp dir, so it may need recreating — the
interface is `keyprobe.py <cols> <rows> '<keys>' <command...>`. Key tokens are
`<up> <down> <left> <right> <tab> <pgup> <pgdn> <home> <end> <enter> <esc>
<space>`, plus literal single characters — `cc` is two `c` presses, and `<c>` is
not a token. Rebuild the release binary before a PTY run; the probe runs the
binary, not the library.

```
python3 /tmp/opencode/keyprobe.py 110 32 '2<enter>cc<left>y' \
    ./target/release/wattdeck --mock --config /tmp/opencode/ctrlcfg.toml
```

That sequence opens the device, picks the third control (AC output), flips it to
`off`, and confirms; the frame shows `-> off`, then `reported off` and the AC
output power dropping.

The probe runs a command, so point it at the binary, not at `make` — wrapping
`make run` in the pty hangs (make forks a shell and cargo writes to the same
pty), and the screen capture comes out empty. Build with `make release`, then
probe the binary.

The same probe captures the wizard: `keyprobe.py 90 12
'QmKDummy<enter>bCiwDummy<enter>' env HOME=… ./target/release/wattdeck setup`.
The keys are read with echo off, so **no captured frame may contain the typed
key text** — that is the assertion, and the frames should show the prompts, the
backend line, and the API's own reply. Use dummy values: a wizard run writes to
the real keyring, and the dummy entries get cleared afterwards. Mock runs capture
the Alerts screen (`4`) to check the hint line naming `wattdeck setup`.

## Live probes

Live probes are separate crates under `/tmp/opencode` that use this project as a
**path dependency**, so they exercise the app's own signing, topics, client
options and `set_payload` — not a reimplementation. `CARGO_TARGET_DIR` keeps
their artefacts out of the repo.

Verified live, in this project:

- GET signing with the query sorted **before** the auth fields → `code 0`; every
  other order → `8521`
- device list, `quota/all` (240 keys), certification (broker host, port 8883,
  `mqtts`)
- the subscribe set: `quota`, `status`, `set_reply` accepted, `get_reply` refused
  (`0x80`), each SUBSCRIBE as its own packet
- the set round trip: full-param commands answered on `set_reply` with `code 0`
  and `data.ack 0`, echoing the command `id`; a command without `id`/`from`
  published and never answered; a partial-param command half-applied
  (`inv.SlowChgWatts` moved to a value that was never sent) and never answered
- history rows, event rows, plain serials in the database = 0, JSON export clean

**Live publishing changes the device.** A probe sends the value the device
reported, i.e. an echo, and re-reads the property map afterwards to prove
nothing moved. Any command that changes the device's state needs the user's
explicit go-ahead.

## Fixtures

`tests/fixtures/quota_all.json` is the masked live property dump of this
account's DELTA 2 Max (240 keys). Tests read it with `include_str!`, so the
normalizer, the capability set, the control `read_key` list and the extra
parameters that must be echoed are all checked against real device output.

`insta` is a dev dependency for UI snapshot tests; the UI is currently covered by
the PTY runs.

## The rules that keep this honest

These are the constraints the code and the tests enforce, in the order they
matter:

1. **Never invent.** No endpoint, signing rule, topic, property name, payload
   field or command is guessed. A control is offered only if the command is
   verified and the property key it writes is present in a real dump. Families
   with no verified command list offer no controls.
2. **Label what is unverified** — in comments, in the docs, and in the UI.
3. **Read-only by default.** Controls need explicit config enable, a pick, a
   value and a confirmation. `--read-only` forces them off. The app never
   auto-commands.
4. **The UI never blocks on network or disk.** Providers and the storage writer
   are separate tasks/threads; channels are bounded; storage sends use
   `try_send` and report a full queue once.
5. **Secrets are never printed.** Serials masked by default and in the
   database, credentials redacted in `Debug`, log file masked, `credentials.toml`
   `chmod 600`.
6. **Unknown properties are preserved** and shown in the raw inspector. Nothing
   the device reported is dropped, and nothing the device did not report is
   displayed.

## Milestones

The project was built in seven milestones, each compiled and tested before the
next, each verified against the live device when it touched the network:

1. CLI, config, logging, the terminal loop, the dashboard skeleton
2. API signing, device list, property map, normalized snapshot
3. MQTT client, topics, payload merge, connection states
4. UI: dashboard, device details, raw inspector, alerts, settings
5. Storage: SQLite writer, sampling, history ranges, analytics, JSON export
6. Analytics buckets, tariff, gap detection, `f`/`u`/`i`, high-output alert
7. Controls: verified command catalog, confirm dialog, replies, capability gating

Each is a commit; the milestone messages name what was verified live.
