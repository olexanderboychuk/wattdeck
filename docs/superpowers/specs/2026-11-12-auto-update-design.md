# Auto-update on launch

Date: 2026-11-12. Status: approved design, not implemented.

## What it does

When `wattdeck` opens, a background task asks GitHub for the latest release. If the
tag is newer than the running build, the task downloads the matching asset, verifies
its SHA256 against `SHA256SUMS.txt` **before** extracting, extracts it, and installs
it over the binary that is running. The running session finishes on the old build;
the app says `restart to apply` in the footer, records the fact on the Alerts
screen, and prints the line to stdout after the terminal is restored.

Four decisions the design rests on, taken by the maintainer:

1. **Silent apply** — no prompt. The download, verify and install happen without a
   user decision, and the session keeps the old build.
2. **Background, after the TUI opens** — zero startup cost. The TUI owns the
   terminal, so the in-app notice is the footer and the Alerts screen, and the
   restart line prints after quit.
3. **Every launch** — no cache file, no check-on-a-timer, no check key.
4. **Config + flag + headless command** — `[update] enabled = true`, `--no-update`
   forces off for one run, `wattdeck update` runs the same code without the TUI.

## Non-goals

No signature verification beyond the published SHA256s, no delta/partial update, no
rollback, no automatic restart, no check on a schedule inside a session, no new key
binding, no last-check timestamp file, no dependency added to `Cargo.toml`.

## Shape

```
run_tui ──spawn──► update::run(cfg, tx)      tokio task: own network, never on the frame path
                    │  GET https://api.github.com/repos/<repo>/releases/latest
                    │  tag_name → (major, minor, patch); current = env!("CARGO_PKG_VERSION")
                    │  newer?  →  pick the asset for this target triple
                    │  download to data_dir/update/<tag>/  (asset + SHA256SUMS.txt)
                    │  SHA256 of the bytes vs the SHA256SUMS line   (verify before extract)
                    │  tar -xzf / tar -xf  →  the new binary in a temp dir
                    │  install over current_exe()
                    ▼
        mpsc<Update> ─► App::release: Option<Status>
                    │  footer: `restart to apply 0.3.0` when Applied
                    │  Alerts screen: AlertKind::Update
                    ▼
run_tui, after restore_terminal(): `updated wattdeck 0.2.0 → 0.3.0 · restart to apply the new build`
```

The repo's invariant survives: **the UI never calls the API.** `update::run` is a
tokio task with its own `reqwest` client; it sends one `Update::Release(Status)`
into the existing bounded channel, and `App` turns it into state. Nothing on the
frame path touches the network, so a slow or absent GitHub answer cannot stall a
frame.

## The rules that must not be broken

- **Mock mode does no network.** `--mock` is documented as "No credentials, no
  network", and the screenshot renderer runs mock runs. So the check is skipped
  whenever the run is mock, whatever the config says — **with one test-only
  exception**: a mock run does the check when `WATTDECK_RELEASE_DIR` is set,
  because that path reads a local directory and touches no network by
  construction. That exception is what makes the footer line and the Alerts
  entry verifiable in a PTY run. `wattdeck update` is always explicit, so it
  runs on its own.
- **Unverified bytes are never run.** The SHA256 of the downloaded asset is
  compared to the `SHA256SUMS.txt` line for that asset name before anything is
  extracted. A mismatch is a refusal with the same words the installer uses
  (`SHA256 mismatch for <asset>: expected …, got …`), and nothing is installed.
- **No sudo, no root, no system directory.** The install target is
  `current_exe()`, and a target directory that is not writable is refused by name,
  the same rule `install.sh` applies.
- **A combination outside the built targets is refused by name.** The mapping is
  the installer's table, and the message names what exists.
- **The rate-limit claim in the FAQ is updated, not glossed.** One unauthenticated
  GitHub API request per launch, GitHub's documented 60/hour per IP, and the
  escape hatches named: `--no-update`, `[update] enabled = false`, and the
  test-only seams `WATTDECK_TAG` + `WATTDECK_RELEASE_DIR`, which skip the API
  request entirely.

## Components

`src/update/mod.rs` — the check. Owns `Status { Available { to }, Applied { from, to },
Failed(String) }`, the API request, the tag parse, the version compare, the target
triple and asset name, the download, the SHA256 verify, and the seams. `Available`
is a transient state: it is traced, never drawn; a silent apply goes straight to
`Applied`.

`src/update/install.rs` — the install. Owns extract (external `tar`), the new
binary's mode, the rename onto `current_exe()`, the Windows `.old` rename, and the
`wattdeck.exe.old` cleanup on the next launch. One purpose: take verified bytes and
put them where the binary lives. It does no network.

Consumers, in the order they run:

- `src/main.rs` — `--no-update`, the `Update` subcommand, the spawn in `run_tui`,
  and the post-`restore_terminal()` line. `wattdeck update` prints what happened
  and exits non-zero on failure; `--debug` writes the check's failures to the log.
- `src/config/settings.rs` — `UpdateConfig { enabled, repo, timeout_seconds }`,
  `Default` (true, `olexanderboychuk/wattdeck`, 10), and `validate()` for
  `timeout_seconds` in 1..=60.
- `src/services/mod.rs` — `Update::Release(Status)`.
- `src/app.rs` — `pub release: Option<Status>`, the `Update::Release` arm,
  `AlertKind::Update`, and the alert on Applied.
- `src/ui/widgets/footer.rs` — the `restart to apply <version>` segment when Applied.

## Version compare

Tags are `v*.*.*` (the release workflow triggers on that pattern), so a numeric
tuple is enough: strip a leading `v`, split on `.`, parse three `u64`. A tag that
does not parse is not newer: traced and skipped, never a guess. `semver` is not
added. `current` is `env!("CARGO_PKG_VERSION")`, which `tests/cli.rs` already pins
to `wattdeck --version`, and the release workflow asserts the tag matches it.

## Asset selection and verification

| OS/arch (`std::env::consts`) | target triple | asset |
| --- | --- | --- |
| `Linux` / `x86_64` | `x86_64-unknown-linux-gnu` | `wattdeck-<tag>-x86_64-unknown-linux-gnu.tar.gz` |
| `Macos` / `aarch64` | `aarch64-apple-darwin` | `wattdeck-<tag>-aarch64-apple-darwin.tar.gz` |
| `Windows` / `x86_64` | `x86_64-pc-windows-msvc` | `wattdeck-<tag>-x86_64-pc-windows-msvc.zip` |
| anything else | refused by name, with the built targets listed | |

The asset is chosen by matching `name` in the release's `assets` array and taking
that asset's `browser_download_url`; `SHA256SUMS.txt` is the asset with that name.
The release job writes each line as `<hash>  <asset>`; the parse takes the first
field of the line whose name matches, and an asset missing from the sums file is a
refusal. `sha2` is already a direct dependency, so no new crate verifies the hash.
External `tar` extracts (`tar -xzf` on unix, `tar -xf` for the zip on Windows); a
machine without `tar` gets a named refusal, the same rule as the installer's
missing-SHA256-tool message.

## Self-replace

The target is `current_exe()`, not a guess about where the installer put things:
the binary the user launched is the binary that gets replaced. `current_exe()` is
the resolved path, so a symlinked `~/.cargo/bin/wattdeck` installs to the file it
points at. That covers `~/.local/bin/wattdeck`, `~/.cargo/bin`,
`target/release/wattdeck` from `make run`, and
`%LOCALAPPDATA%\wattdeck\bin\wattdeck.exe`.

- **unix**: write the new binary as `<dir>/wattdeck.new` in the target's own
  directory (same filesystem, so the rename is atomic), `chmod 755`, then rename
  over `wattdeck`. The running process keeps its old inode and finishes its session
  on the old build — which is why the message is `restart`, not `reloaded`.
- **Windows**: rename `wattdeck.exe` → `wattdeck.exe.old` (a rename of a running
  executable is allowed), move the new `wattdeck.exe` into place, and delete
  `wattdeck.exe.old` at the next launch, before the terminal is set up. A leftover
  `.old` that cannot be deleted is traced and left; it never blocks the check.
- **Labelled unverified**: no Windows machine has run wattdeck, so the rename
  order, `bsdtar`'s zip handling and the `.old` cleanup are claimed, not tested.
  The CI Windows job runs the Rust tests; the integration test that installs a
  binary is skipped by name on Windows.

## Messages

Exact strings, so the tests can assert them:

- footer, when Applied: `restart to apply 0.3.0`
- Alerts screen on Applied: `updated wattdeck 0.2.0 → 0.3.0 · restart to apply`
- stdout after `restore_terminal()`: `updated wattdeck 0.2.0 → 0.3.0 · restart to apply the new build`
- `wattdeck update`, current: `wattdeck 0.2.0 is the latest build`
- `wattdeck update`, installed: `updated wattdeck 0.2.0 → 0.3.0 · restart to apply the new build`
- `wattdeck update`, failure: `update check failed: <reason>`, exit non-zero

Startup failures are traced to the log and not alerted: a rate-limit refusal on
every launch must not fill the Alerts screen. `--debug` shows them.

## Config and CLI

```toml
[update]
enabled = true                       # false, or --no-update, means no network
repo = "olexanderboychuk/wattdeck"   # the fork escape hatch
timeout_seconds = 10                 # validate 1..=60
```

CLI surface gains two entries:

```
wattdeck --no-update
wattdeck update      # check, verify, install, print what happened
```

Precedence: `--no-update` forces `enabled = false` for the run, the same shape as
`--read-only` forcing controls off. Mock mode skips the check regardless.

## Tests

Unit tests, in `src/update/`, no network, no terminal, no device:

- version compare: `v0.3.0` > `0.2.0`, `0.2.1` > `0.2.0`, `0.2.0` not newer,
  `v1.0.0` > `0.99.9`, a tag that does not parse is not newer.
- `SHA256SUMS.txt` parse: the matching line, an asset missing from the file, a
  mismatch.
- target triple table: the three built triples, and the by-name refusal for
  `Linux`/`aarch64`.
- install path (unix): a `tar.gz` built in a temp dir containing a
  `#!/bin/sh` marker binary, verified, extracted, installed over a temp target;
  assert the target is the marker, mode 755, and that a running "old" file keeps
  working (the rename, not a copy).

Integration, `tests/update.rs`, in the `tests/cli.rs` style: copy the built binary
to a temp dir, build a fake release dir — a `tar.gz` containing a `#!/bin/sh`
marker script that prints `wattdeck 9.9.9` and exits, mode 755, plus
`SHA256SUMS.txt` computed from that asset — then run
`WATTDECK_RELEASE_DIR=… WATTDECK_TAG=v9.9.9 <tmp>/wattdeck update`. Assert the
restart line is printed and that `<tmp>/wattdeck --version` now prints the
marker's `wattdeck 9.9.9`, which is the observable proof that the bytes were
verified, extracted and installed. Skipped by name on Windows, stated in the test
comment.

`make test` and CI run all of this on all three OSes, offline, because the seams
skip the API request. The API request itself is verified by one live probe run,
described in `docs/DEVELOPMENT.md`, against the real latest-release endpoint.

## Docs that change

- `README.md`: `wattdeck update` and `--no-update` in the CLI surface; the FAQ
  rate-limit paragraph rewritten for one request per launch; the troubleshooting
  line for `no tar` and for the rate limit.
- `docs/CONFIGURATION.md`: the `[update]` table and the validation list.
- `docs/ARCHITECTURE.md`: the update task in the shape diagram, as another task
  that talks to the network and never touches the UI.
- `docs/DEVELOPMENT.md`: the seams, the new integration test, the live probe.
- `SECURITY.md`: the out-of-scope line stays true as written — the app verifies
  downloads against the published SHA256s and nothing beyond them.
- `docs/KEYBOARD_SHORTCUTS.md`: no change. There is no new key.

## Verification at the end

`make` (build, unit tests, `fmt --check`, `clippy --all-targets`), `make test`,
the PTY run on a copy of the release binary in a temp dir with
`WATTDECK_RELEASE_DIR` + `WATTDECK_TAG` pointing at an asset containing a marker
binary (mock mode, seams, so no network), showing the footer line, the Alerts
entry, and the stdout line after quit, a live `wattdeck update` against the real
latest release, and the install over the installed binary.
