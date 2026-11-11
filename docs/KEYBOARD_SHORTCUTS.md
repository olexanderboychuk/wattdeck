# Keyboard shortcuts

Every key is resolved through a `KeyMap`, so nothing in the app is hard-wired to
a key, and everything below is remappable in `[keys]` in `config.toml`. The `?`
screen lists the keys in use, using the current mapping.

## Screens

| Key | Action |
| --- | --- |
| `1` | dashboard |
| `2` | device list |
| `3` | analytics |
| `4` | alerts |
| `5` | settings |
| `?` | help |
| `tab` | next screen |
| `shift+tab` | previous screen |

## Navigation

| Key | Action |
| --- | --- |
| `up` / `down` | previous / next device |
| `left` / `right` | previous / next panel — and, on the controls panel, step the selected control's value |
| `pgup` / `pgdn` | scroll a list |
| `home` / `end` | top / bottom of a list |
| `enter` | open the selected device; and confirm a pending control |
| `esc` | back; and cancel a pending control |

Navigation keys are read-only: no navigation key sends a command.

## Controls (device commands)

Controls appear only with `[controls] enabled = true`, and only for a family
with verified commands (the DELTA 2 family). `--read-only` disables them.

| Key | Action |
| --- | --- |
| `c` | next control |
| `shift+c` | previous control |
| `left` / `right` | step the selected control's value (clamped to the range the command accepts; a toggle flips on/off) |
| `enter` / `y` | confirm: send the command the dialog shows |
| `n` / `esc` | cancel: the command is dropped, nothing is sent |

The confirmation dialog sits at the centre of the frame with a warning border,
naming the control, the masked serial, the parameter and the value. A device
that is not reporting refuses the confirmation and keeps the pending value, with
the status line saying nothing was sent.

Value steps: 5 % for percentages, 100 W for power, 60 min for the standby
timeout, 30 s for the screen timeout.

## Display and diagnostics

| Key | Action |
| --- | --- |
| `r` | request a refresh |
| `i` | show the raw serial (masked by default) |
| `u` | temperature unit, °C / °F |
| `f` | frame rate, cycles 30 → 60 → 15 |
| `s` | switch the mock scenario (mock mode only) |
| `q` | quit |

Range keys (`pgup` / `pgdn` / `home` / `end`, `left` / `right`) scrub the
analytics time range; the bucket size is the `BUCKETS` panel (per minute, 5
minutes, hourly, daily, weekly).
