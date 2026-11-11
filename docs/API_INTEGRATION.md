# API integration

Everything here is a fact this project verified, a fact two independent
integrations agree on, or an item the app does not use. The labels are the same
as in the findings file: `[DOC]` official developer-portal documentation,
`[MULTI]` two or more independent implementations agree, `[SINGLE]` one
implementation or a verbatim capture, `[LIVE]` probed against the live API from
this project, and **not verified** for the things that could not be confirmed.
Nothing in the code is an invented endpoint, topic, property name or command.

## Credentials and hosts

The developer portal issues an access key and a secret key for your account.
The devices a key can see are the devices bound to that account; shared devices
are not visible `[DOC]`.

- EU accounts use `https://api-e.ecoflow.com` `[MULTI]`. This project uses that
  host, and it is the only host verified for this account `[LIVE]`. Other region
  hosts (`api`, `api-a`, `api-j`, `api-r`, `api-cn`) exist in integration
  config files `[MULTI]` and are not verified here. `base_url_override` in the
  config is the escape hatch.
- `GET /iot-open/sign/certification` `[DOC]` returns
  `certificateAccount`, `certificatePassword`, `url`, `port`, `protocol`. Live
  for this account `[LIVE]`: `url: mqtt-e.ecoflow.com`, `port: "8883"`,
  `protocol: "mqtts"`. The username is `certificateAccount` — a credential, and
  the first path element of every developer topic.

## Signing

Headers on every request: `accessKey`, `nonce` (6 digits), `timestamp` (UTC ms),
`sign`. `sign` is lowercase hex HMAC-SHA256 keyed with the secret key `[DOC]`.

The canonical string is sorted `k=v` pairs, nested keys joined with dots, array
elements as `key[i]`, booleans as `true`/`false`, no URL encoding, then
`&accessKey=..&nonce=..&timestamp=..` appended `[DOC]`. This project reproduces
the official test vector byte for byte in a unit test
(`src/api/signing.rs::official_test_vector`) `[DOC]`.

**GET signing rule — probed live on 2026-11-10** `[LIVE]`: the request's
parameters, sorted, go **before** the auth fields, in the same canonical shape
as a POST body.

| Canonical string | Result |
| --- | --- |
| `accessKey=..&nonce=..&timestamp=..` (no params, `/device/list`) | `code 0` |
| `sn=…&accessKey=..&nonce=..&timestamp=..` (`/quota/all?sn=…`) | `code 0`, 240 property keys |
| `accessKey=..&nonce=..&timestamp=..&sn=…` (auth first) | `code 8521` "signature is wrong" |
| `sn=…&accessKey=…` (query after auth) | `code 8521` |
| path + query, path + auth | `code 8521` |

A GET with no parameters signs the auth fields alone. `quota/all` without `sn`
returns `code 1006 "sn can not be empty"` `[LIVE]`.

The nonce only has to be unique per request, so it is derived from the
sub-second clock (`timestamp_subsec_nanos() % 1_000_000`, printed as `%06d`). No
random crate.

## Endpoints the app uses

| Endpoint | Shape | Status |
| --- | --- | --- |
| `GET /iot-open/sign/device/list` | list of `{sn, online, productName}`, plus `deviceName` on some entries | `[DOC]` + `[LIVE]` |
| `GET /iot-open/sign/device/quota/all?sn=…` | flat map of dotted keys, string values | `[DOC]` + `[LIVE]`, 240 keys for the DELTA 2 Max |
| `GET /iot-open/sign/certification` | MQTT credentials | `[DOC]` + `[LIVE]` |

`deviceName` is the user's nickname and equals the serial when the device was
never renamed, so the app never prints it unmasked `[LIVE]`.

`POST /iot-open/sign/device/quota` and `PUT /iot-open/sign/device/quota` are
documented `[DOC]` and are **not used** by this app: the property map comes from
`quota/all`, which is cheaper and complete.

## MQTT

- Broker port `8883`, TLS, username `certificateAccount`, password
  `certificatePassword` `[DOC]` + `[MULTI]`.
- MQTT 3.1.1 `[MULTI]`. TLS verification on by default `[MULTI]`;
  `verify_broker_cert` is the config key, and `tls_override` exists only for a
  local test broker, never for the real one.
- Keepalive 30 s, inside the 15–60 s range implementations use `[MULTI]`.
  Clean session, QoS 1 for subscribe and publish `[MULTI]`.
- Client id is fixed and derived from the account name. The broker tolerates
  about 10 unique ids per day per key; exceeding it gives CONNACK rc 5 and only
  new keys fix it `[SINGLE]`. Ports `443` and `8083` are **not verified**, and
  the app does not use them.

### Topics

Developer path `[DOC]` + `[MULTI]`: `/open/{certificateAccount}/{sn}/{kind}`
with kinds `get`, `get_reply`, `set`, `set_reply`, `quota`, `status`.

**Subscribe set, verified live on 2026-11-10** `[LIVE]`: `quota`, `status` and
`set_reply` each get `SUBACK` success; `get_reply` is refused (`SUBACK 0x80`).
EcoFlow refuses `SUBSCRIBE /app/device/property/<sn>` with SUBACK 128 as of
2026-07-26 `[SINGLE]`. A combined multi-topic `SUBSCRIBE` fails, so each topic
goes out as its own packet.

Consequences the app is built around: the full property map comes from REST (the
backfill), `quota` carries the partial blocks the device pushes, and the app
publishes `latestQuotas` on `get` on the refresh cadence, because devices slow
their pushes when nobody polls.

### Payload shapes

Full snapshot (`get_reply`) `[SINGLE]`, verbatim capture for a DELTA 2 Max
(productType 81):

```json
{"id":874179349,"version":"1.0","operateType":"latestQuotas","moduleSn":"…",
 "code":"0","data":{"sn":"…","productType":81,"model":1,"online":1,
 "quotaMap":{"bms_bmsStatus.soc":"…", …}}}
```

Partial block on `quota` `[SINGLE]`:

```json
{"moduleType":1,"needAck":0,"id":2935860,"time":2935860,"version":"1.0",
 "typeCode":"pdStatus","params":{ … changed keys, unprefixed … }}
```

The `typeCode` gives the key prefix (`pdStatus` → `pd.`, `invStatus` → `inv.`,
`bmsStatus` → `bms_bmsStatus.`, `emStatus` → `bms_emsStatus.`,
`mpptStatus` → `mppt.`), taken from the meta keys in a real merged dump
`[SINGLE]`.

Status `[DOC]`: `{"id":"268020839","params":{"status":0},"timestamp":…,
"version":"1.0"}`.

Offline (RIVER series capture) `[SINGLE]`: `{"header":[{"code":"-2"}]}`.

An envelope `online: 0` with telemetry streaming in does not make the app show
the device offline: the app sets `snap.online = true` when the device is pushing
data. Empty arrays (`cellVol: []`) are not measurements.

## Set commands

Envelope the live device answers `[LIVE]`:

```json
{"from":"wattdeck","id":999900001,"version":"1.0","moduleType":2,
 "operateType":"upsConfig","params":{"maxChgSoc":90}}
```

Publish `/open/{certificateAccount}/{sn}/set`, QoS 1. The device answers on
`set_reply` and echoes the `id` `[LIVE]`: the `set_reply` carries
`{"code":"0","moduleType":2,"data":{"ack":0},"operateType":"upsConfig",
"version":"1.0","id":999900001,"time":…}`.

A command published **without** `id` and `from` was accepted by the broker and
**never answered** `[LIVE]`. A command published with only the parameter being
set was accepted, half-applied (`inv.SlowChgWatts` moved to a value that was
never sent) and never answered `[LIVE]`. The full parameter set the verified
commands carry is answered with `code 0` inside two seconds, with no setting
changed when the parameters echo the device's own values `[LIVE]`. So the app
sends `from`, `id`, `version`, `moduleType`, `operateType` and the complete
`params`, each extra parameter echoed from the value the device reported (the
sentinels `-1` / `255` the integrations pass for "leave as is" when the device
did not report it) `[MULTI]`.

`from` carries the app's own name. The integrations pass their own value for it
(`"HomeAssistant"`, `"iOS"`, `"Android"`, `"Web"`) `[DOC]`, and the live DELTA 2
Max answered this app's string `[LIVE]`, so `from` is not part of the verified
command form. Renaming the app changes this string and nothing else about the
payload.

Reply codes the app reports: `0` accepted `[DOC]` + `[LIVE]`, `-1` device not
owned `[DOC]`, `-2` device offline `[DOC]`, `-3` the value is outside the range
the control accepts (the app's own check, before publishing), `-4` the broker
refused the publish, `-5` the device has reported no settings, so nothing is
sent.

Not verified, and not used: `module_update`, `ackSeg`, `sdpsn`, `deviceType`,
the `cmdId`/`cmdFunc` envelope, ports 443 and 8083.

## Error codes seen live

`0` success, `8521` signature is wrong, `1006` (`sn can not be empty` /
`current device is not allowed to get device info` for the offline RIVER mini)
`[LIVE]`. The app shows refusals as alerts naming the masked serial, and reports
the same refusal from the same device once, not on every poll.
