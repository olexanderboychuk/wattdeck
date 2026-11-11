# Device support

The app knows a device by its **property dialect**, not by marketing name.
Dialect decides which keys mean what; capabilities decide which panels and
controls the device is offered. A device the app does not know is shown as
"Unknown device" with the generic capability set, and its raw keys are still on
screen.

## Families and dialects

| Family | Keys | Notes |
| --- | --- | --- |
| DELTA 2 family | `bms_bmsStatus.*`, `bms_emsStatus.*`, `pd.*`, `inv.*`, `mppt.*` | Verified live: 240 keys from a DELTA 2 Max |
| DELTA Pro family | `bmsMaster.*`, `ems.*` | From public device dumps, not verified in this project |
| DELTA Pro 3 | flat camelCase, no prefix: `bmsBattSoc`, `powInSumW` | Public dumps, not verified here |
| RIVER | `bms_bmsStatus.*` subset, `pd.*` | Live device is offline; properties refused (`code 1006`) |
| Power Stream | `powGet*` per-channel power keys | Not verified in this project |
| Smart Plug | `powGet*` | Not verified in this project |
| Unknown | generic | Name lookup falls back here |

`powGet*` keys are per channel (`powGetPvH`, `powGetPvL`, `powGetAcHvOut`,
`powGetAcLvOut`, `powGetAcLvTt30Out`, `powGet4p81`, `powGet4p82`, `powGet5p8`,
`powGetQcusb1`, `powGetQcusb2`, `powGetTypec1`, `powGetTypec2`) and are summed
by the derive step, so a multi-channel device reports one total.

## Known product types

| productType | Model | Family | Verification |
| --- | --- | --- | --- |
| 81 | DELTA 2 Max | DELTA 2 | **verified live** (this account: `productName "DELTA 2 Max"`, `bms_bmsStatus.productType 81`) |
| 80 | DELTA 2 | DELTA 2 | public dumps |
| 72 | RIVER 2 Pro | RIVER | public dumps |
| 71 | RIVER 2 Max | RIVER | public dumps |
| 70 | RIVER 2 | RIVER | public dumps |
| — | RIVER mini | RIVER | verified live by name; product type not verified, recorded as 0 |
| 15 | DELTA Pro | DELTA Pro | public dumps |
| 64 | DELTA Pro 3 | DELTA Pro 3 | public dumps |
| 67 | DELTA Mini | DELTA Pro | public dumps |
| 46 | GLACIER | unknown | public dumps |
| 76 | POWER STREAM | Power Stream | public dumps |
| 77 | Smart Plug | Smart Plug | public dumps |

The REST device list reports `productName` strings, not product types, so the
name lookup fills the family before any quota data arrives. A device that
reports nothing is never shown with values it did not report.

## Capabilities

`DeviceCapabilities` (`src/devices/capabilities.rs`) is a named set of flags:
battery monitoring, health, cells, temperature, solar input, AC input, DC
input, AC output, DC output, USB output, AC output control, DC output control,
charge limit control, extra battery, grid connection, energy history. Panels
appear only when the device reports the capability; a control appears only when
its capability is set **and** the command is verified for that family. A
PowerStream device (productType 76) is offered no controls, because there is no
verified command list for it here.

## Controls: what is verified

The eight DELTA 2 commands. `moduleType` / `operateType` / parameter and the
property key the device reports the setting under are agreed by two independent
integrations, and the key is present in the live 240-key dump. The extra
parameters are the ones the verified command carries; they are echoed from the
device's own reported values (the `-1` / `255` "as is" sentinels the
integrations pass when a key is not reported).

| Control | Command | Range | Device reports it as | Extra params |
| --- | --- | --- | --- | --- |
| charge limit | `moduleType 2`, `upsConfig`, `maxChgSoc` | 60–100 % | `bms_emsStatus.maxChargeSoc` | — |
| discharge limit | `2`, `dsgCfg`, `minDsgSoc` | 0–100 % | `bms_emsStatus.minDsgSoc` | — |
| AC output | `3`, `acOutCfg`, `enabled` | on/off | `inv.cfgAcEnabled` | `out_voltage -1`, `out_freq 255`, `xboost = inv.cfgAcXboost` |
| DC output | `1`, `dcOutCfg`, `enabled` | on/off | `pd.dcOutState` | — |
| AC charge power | `3`, `acChgCfg`, `slowChgWatts` | 200–2400 W | `inv.SlowChgWatts` | `fastChgWatts = inv.FastChgWatts`, `chgPauseFlag = inv.chgPauseFlag` |
| standby timeout | `1`, `standbyTime`, `standbyMin` | 0–720 min | `pd.standbyMin` | — |
| beep | `1`, `quietCfg`, `enabled` | on/off | `pd.beepMode` | — |
| screen timeout | `1`, `lcdCfg`, `delayOff` | 10–600 s | `pd.lcdOffSec` | `brighLevel = pd.brightLevel` |

Live values for this account's DELTA 2 Max: `maxChargeSoc 100`,
`minDsgSoc 5`, `cfgAcEnabled 1`, `cfgAcXboost 1`, `SlowChgWatts 400`,
`FastChgWatts 2400`, `chgPauseFlag 0`, `dcOutState 1`, `standbyMin 30`,
`beepMode 0`, `lcdOffSec 300`, `pd.brightLevel 100`.

**Every other family offers no controls.** Commands for DELTA Pro, DELTA Pro 3,
RIVER, Power Stream and Smart Plug are documented in integration sources, but
they are not verified in this project, so the app does not offer them: an
unverified command is never sent to a device.

## Property names that do not exist

The `ups_*`, `emwattsum`, `bmsCellInfo`, `acOutOn`, `battery_charge_bias` and
`ac_out_on_off` names that appear in some specs and issue reports are **not**
property keys on the developer path. The app uses the real dotted names
(`bms_bmsStatus.*`, `bms_emsStatus.*`, `pd.*`, `inv.*`, `mppt.*`). The set of
keys the device reports is on screen in the raw inspector, so the names can be
checked against the device, not against a spec.

## Blocks, not increments

MQTT blocks carry current measurements, not deltas. `pd.invInWatts` /
`pd.invOutWatts` fill the AC fields only when `inv` has not reported them.
`pd.chgPowerAC` / `pd.dsgPowerAC` are cumulative Wh, divided by 1000 for kWh.
Empty arrays (`bms_emsStatus.cellVol: []`) are not measurements and are not
plotted.

## Mock devices

The mock provider simulates the DELTA 2 dialect, including the settings the
controls write, so the control flow, the confirm dialog, the reported rows and
the replies are exercised without a device. Mock screens show `MOCK`, mock
serials are placeholders, and mock runs never write history.
