# Security policy

## What is sensitive here

- **Your API keys are account-scoped.** An `Access Key` and a `Secret Key` identify
  your developer app **and your account**: anyone holding them can query and command
  your devices, not just one device. Keys are not device-scoped.
- **Device serial numbers** identify your devices and are enough to address them
  over MQTT. wattdeck masks serials in the UI, in the history database, and in the
  log by default; a device serial becomes a `Secret` only when it is registered.
- **Your history file** contains your device data. It lives in the user data
  directory (`~/.local/share/wattdeck` on Linux, `~/Library/Application Support/wattdeck`
  on macOS, `%LOCALAPPDATA%\wattdeck` on Windows). It is not uploaded anywhere.

## Where credentials live

- On Linux and macOS, in the OS keyring under the service name `wattdeck`
  (the freedesktop Secret Service on Linux, the Keychain on macOS). `wattdeck setup`
  writes them; the app reads them at start. The keyring's own access rules apply.
- On Windows, where no supported keyring backend is available, in
  `wattdeck\credentials.toml` inside the config directory (`%APPDATA%\wattdeck`).
  wattdeck sets mode 600 on unix; it does not set Windows permissions, so the
  file inherits the access rules of your user profile directory. On unix it also
  warns in the log if the file is group- or world-readable.
- Neither file is committed, and `wattdeck` never prints key values: the log
  masks registered secrets, and the `Credentials` debug output is redacted.

## If your keys may have leaked

Rotate them in the EcoFlow developer portal. Rotation is a portal action, not an
app action; wattdeck does not perform it. Then run `wattdeck setup` again.

## Reporting a vulnerability

Contact the maintainer privately, through the repo's Security tab, by opening a
private issue, or by direct message. Do not open a public issue containing an API
key, a serial number, a token, or a log file with unmasked values. If you are
reporting a crash with a log, run `wattdeck --debug > report.log` and check the
file for your credentials first.

## Out of scope for this project

wattdeck is not a security boundary. It does not encrypt the history file, it
does not sign its binaries, and it does not verify downloads against any
signature beyond the SHA256 checksums published in the release. If your threat
model includes an attacker who can write to your config directory, the file-mode
protection is not a defense.

The launch update check verifies the downloaded asset against the SHA256 published
in the release, extracts it with the system `tar`, and installs it into the
directory the binary already lives in: no sudo, no elevated write, no code signing,
and no verification beyond the published checksums.
