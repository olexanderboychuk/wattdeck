//! Device controls.
//!
//! Every control here is a command that is *verified* — agreed between two or more
//! independent integrations, or captured verbatim from the EcoFlow app — and whose
//! matching property key is present in a real property dump. Nothing is invented.
//!
//! Controls are inert by default: `ControlConfig::enabled` must be true in the
//! config, the app asks for confirmation before sending, and the command is sent to
//! a device the app has confirmed is online.

use serde_json::Value;

use crate::devices::capabilities::DeviceCapabilities;
use crate::devices::registry::Family;

/// What a control writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    /// On/off, sent as 1/0.
    Toggle,
    /// A number with the limits the device accepts.
    Number {
        min: i64,
        max: i64,
        unit: &'static str,
    },
}

/// A param the command carries besides the one the user picked. A live DELTA 2 Max
/// acknowledged the full command shape the integrations send (`acOutCfg` with
/// `out_voltage`/`out_freq`/`xboost`, `acChgCfg` with `fastChgWatts`/`chgPauseFlag`,
/// `lcdCfg` with `brighLevel`) and answered every one. A command with only the picked
/// param was published, half-applied (`inv.SlowChgWatts` moved to a value that was
/// never sent), and never answered — so the params the verified command carries are
/// all sent, at the value the device reports, which leaves them unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Extra {
    /// The device's own current value for a property key, so the command writes the
    /// param back as the device has it. `255`, the "as is" sentinel the integrations
    /// pass, is used when the device did not report the key.
    Key(&'static str),
    /// A fixed value: the sentinel the integrations pass for "do not change".
    Fixed(i64),
}

impl Extra {
    fn read(&self, raw: &std::collections::HashMap<String, Value>) -> i64 {
        match self {
            Extra::Key(k) => raw
                .get(*k)
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                .unwrap_or(255),
            Extra::Fixed(v) => *v,
        }
    }
}

/// One command the app can send.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Control {
    /// Stable name used in config, tests and the UI.
    pub id: &'static str,
    /// Label shown in the confirmation dialog.
    pub label: &'static str,
    pub module_type: u8,
    pub operate_type: &'static str,
    /// The parameter the command carries, and the limits it accepts.
    pub param: &'static str,
    pub kind: ValueKind,
    /// Property key the device reports this setting under, so the dialog can show
    /// the value the device currently has. Verified against a live property dump.
    pub read_key: &'static str,
    /// The capability the device must report for this control to be offered.
    pub requires: Capability,
    /// The params the verified command carries besides `param`.
    pub extras: &'static [(&'static str, Extra)],
}

/// Which feature a control depends on. Named, not a function pointer, so the
/// control list stays comparable and copyable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Capability {
    ChargeLimit,
    AcOutput,
    DcOutput,
    AcInput,
    Battery,
    History,
}

impl Control {
    /// Whether a device with these capabilities can take this control. A device
    /// that does not report the feature is never offered it.
    pub fn available(&self, caps: DeviceCapabilities) -> bool {
        match self.requires {
            Capability::ChargeLimit => caps.charge_limit_control,
            Capability::AcOutput => caps.ac_output_control,
            Capability::DcOutput => caps.dc_output_control,
            Capability::AcInput => caps.ac_input,
            Capability::Battery => caps.battery_monitoring,
            Capability::History => caps.energy_history,
        }
    }
}

impl Control {
    /// The command payload, in the envelope the live device answers: `from`, `id`,
    /// `version`, `moduleType`, `operateType`, and the full `params` the verified
    /// command carries. `raw` is the property map the device reported, used for the
    /// params that are settings the device has (`inv.cfgAcXboost`, `inv.FastChgWatts`,
    /// `inv.chgPauseFlag`, `pd.brightLevel`), so the command leaves them as they are.
    pub fn payload(
        &self,
        value: i64,
        id: u64,
        raw: &std::collections::HashMap<String, Value>,
    ) -> Value {
        let mut params = serde_json::Map::new();
        params.insert(self.param.to_string(), Value::from(value));
        for (key, extra) in self.extras {
            params.insert((*key).to_string(), Value::from(extra.read(raw)));
        }
        serde_json::json!({
            // `from` and `id` are the fields a live DELTA 2 Max answers: verified on
            // 2026-11-10, the `set_reply` echoes the `id` the command carried. A
            // command published without them was never answered.
            "from": "wattdeck",
            "id": id,
            "version": "1.0",
            "moduleType": self.module_type,
            "operateType": self.operate_type,
            "params": params
        })
    }

    /// Whether a value is inside the range this control accepts.
    pub fn accepts(&self, value: i64) -> bool {
        match self.kind {
            ValueKind::Toggle => value == 0 || value == 1,
            ValueKind::Number { min, max, .. } => value >= min && value <= max,
        }
    }

    /// The value in words for the dialog.
    pub fn render(&self, value: i64) -> String {
        match self.kind {
            ValueKind::Toggle => {
                if value == 1 {
                    "on".to_string()
                } else {
                    "off".to_string()
                }
            }
            ValueKind::Number { unit, .. } => format!("{value} {unit}"),
        }
    }
}

/// The controls the app can offer for a family. Only the ones listed here exist,
/// and each is gated by the capability the device reports.
pub fn for_family(family: Family) -> &'static [Control] {
    match family {
        Family::Delta2 => DELTA_2,
        // The commands for the other families are not verified for this app, so no
        // control is offered: an unverified command is never sent to a device.
        _ => &[],
    }
}

/// DELTA 2 / DELTA 2 Max controls. Verified by agreement between two independent
/// integrations (tolwi `delta2_max.py`, iobroker `ef_delta2max_data.js`), each with
/// the property key it writes. Live values for this account's DELTA 2 Max:
/// `maxChargeSoc 100`, `minDsgSoc 5`, `cfgAcEnabled 1`, `dcOutState 1`,
/// `SlowChgWatts 400`, `standbyMin 30`, `beepMode 0`, `lcdOffSec 300`.
const DELTA_2: &[Control] = &[
    Control {
        id: "charge_limit",
        label: "charge limit",
        module_type: 2,
        operate_type: "upsConfig",
        param: "maxChgSoc",
        kind: ValueKind::Number {
            min: 60,
            max: 100,
            unit: "%",
        },
        read_key: "bms_emsStatus.maxChargeSoc",
        requires: Capability::ChargeLimit,
        extras: &[],
    },
    Control {
        id: "discharge_limit",
        label: "discharge limit",
        module_type: 2,
        operate_type: "dsgCfg",
        param: "minDsgSoc",
        kind: ValueKind::Number {
            min: 0,
            max: 100,
            unit: "%",
        },
        read_key: "bms_emsStatus.minDsgSoc",
        requires: Capability::ChargeLimit,
        extras: &[],
    },
    Control {
        id: "ac_output",
        label: "AC output",
        module_type: 3,
        operate_type: "acOutCfg",
        param: "enabled",
        kind: ValueKind::Toggle,
        read_key: "inv.cfgAcEnabled",
        requires: Capability::AcOutput,
        // `acOutCfg` carries the whole AC output configuration: the sentinels the
        // integrations pass mean "leave as is", and `xboost` is echoed from the
        // value the device reports.
        extras: &[
            ("out_voltage", Extra::Fixed(-1)),
            ("out_freq", Extra::Fixed(255)),
            ("xboost", Extra::Key("inv.cfgAcXboost")),
        ],
    },
    Control {
        id: "dc_output",
        label: "DC output",
        module_type: 1,
        operate_type: "dcOutCfg",
        param: "enabled",
        kind: ValueKind::Toggle,
        read_key: "pd.dcOutState",
        requires: Capability::DcOutput,
        extras: &[],
    },
    Control {
        id: "ac_charge_watts",
        label: "AC charge power",
        module_type: 3,
        operate_type: "acChgCfg",
        param: "slowChgWatts",
        // The device reports 400 W slow / 2400 W fast for this model; the accepted
        // span is the range it exposes, not a guess.
        kind: ValueKind::Number {
            min: 200,
            max: 2400,
            unit: "W",
        },
        read_key: "inv.SlowChgWatts",
        requires: Capability::AcInput,
        // The device answers `acChgCfg` only when the fast charge limit and the
        // charge-pause flag travel with the slow limit. Both are echoed from the
        // values the device reports, so the command changes the picked limit only.
        extras: &[
            ("fastChgWatts", Extra::Key("inv.FastChgWatts")),
            ("chgPauseFlag", Extra::Key("inv.chgPauseFlag")),
        ],
    },
    Control {
        id: "standby_minutes",
        label: "standby timeout",
        module_type: 1,
        operate_type: "standbyTime",
        param: "standbyMin",
        kind: ValueKind::Number {
            min: 0,
            max: 720,
            unit: "min",
        },
        read_key: "pd.standbyMin",
        requires: Capability::History,
        extras: &[],
    },
    Control {
        id: "beep",
        label: "beep",
        module_type: 1,
        operate_type: "quietCfg",
        param: "enabled",
        kind: ValueKind::Toggle,
        read_key: "pd.beepMode",
        requires: Capability::Battery,
        extras: &[],
    },
    Control {
        id: "lcd_timeout",
        label: "screen timeout",
        module_type: 1,
        operate_type: "lcdCfg",
        param: "delayOff",
        // The device reports 300 s; the app's own range is the documented one.
        kind: ValueKind::Number {
            min: 10,
            max: 600,
            unit: "s",
        },
        read_key: "pd.lcdOffSec",
        requires: Capability::Battery,
        // `lcdCfg` carries the brightness too; echoing the value the device reports
        // leaves the brightness alone.
        extras: &[("brighLevel", Extra::Key("pd.brightLevel"))],
    },
];

/// The control a user has picked, together with the value to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub control: &'static Control,
    pub value: i64,
}

impl Pending {
    /// The one-line description shown in the confirmation dialog.
    pub fn describe(&self) -> String {
        format!(
            "{} = {}",
            self.control.label,
            self.control.render(self.value)
        )
    }
}

/// The first command id. The base and the range come from the integration that
/// uses them (`999900000 + random(10000..99999)`); the app counts up from the base
/// so every command in a session has its own id, which the device echoes back.
pub const BASE_ID: u64 = 999_900_001;

/// The command envelope the live device answers: `{"from","id","version",
/// "moduleType","operateType","params"}`. Verified on 2026-11-10 against this
/// account's DELTA 2 Max: seven commands of this shape, each with the value the
/// device reported, were published on `/open/{account}/{sn}/set` and every one was
/// answered on `set_reply` with `code 0` and `data.ack 0`. A command published
/// without `id` and `from` was published and never answered.
pub fn set_payload(
    control: &Control,
    value: i64,
    id: u64,
    raw: &std::collections::HashMap<String, Value>,
) -> String {
    control.payload(value, id, raw).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::devices::capabilities::DeviceCapabilities;
    use crate::devices::registry::lookup;

    #[test]
    fn delta2_controls_are_the_verified_set() {
        let controls = for_family(Family::Delta2);
        let ids: Vec<&str> = controls.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "charge_limit",
                "discharge_limit",
                "ac_output",
                "dc_output",
                "ac_charge_watts",
                "standby_minutes",
                "beep",
                "lcd_timeout"
            ]
        );
    }

    #[test]
    fn a_family_with_no_verified_commands_offers_nothing() {
        assert!(for_family(Family::River).is_empty());
        assert!(for_family(Family::Unknown).is_empty());
        // A product with no verified commands gets no control at all, so a command
        // is never sent to a device the app has not verified.
        assert!(for_family(lookup(76).family).is_empty(), "PowerStream");
        // And within the family that does have commands, the capability decides: a
        // device that does not report AC output control is not offered the switch.
        let mut caps = DeviceCapabilities::DELTA_2_FAMILY;
        caps.ac_output_control = false;
        let ac = DELTA_2
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        assert!(
            !ac.available(caps),
            "no verified AC output control, no switch"
        );
        let charge = DELTA_2
            .iter()
            .find(|c| c.id == "charge_limit")
            .expect("charge limit");
        assert!(charge.available(caps), "the charge limit is still reported");
    }

    /// The live property map this account's DELTA 2 Max reports, the values a
    /// command has to echo to leave the untouched settings alone.
    fn live() -> std::collections::HashMap<String, Value> {
        serde_json::from_str(include_str!("../../tests/fixtures/quota_all.json")).expect("fixture")
    }

    #[test]
    fn payloads_match_the_form_the_live_device_answers() {
        // The agreed command shape plus the fields the live DELTA 2 Max acknowledged
        // on 2026-11-10, and the live values this account's device reports.
        let raw = live();
        let c = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "charge_limit")
            .expect("charge limit");
        assert_eq!(
            set_payload(c, 90, BASE_ID, &raw),
            r#"{"from":"wattdeck","id":999900001,"moduleType":2,"operateType":"upsConfig","params":{"maxChgSoc":90},"version":"1.0"}"#
        );
        let ac = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        let p =
            serde_json::from_str::<Value>(&set_payload(ac, 1, BASE_ID + 1, &raw)).expect("json");
        assert_eq!(p["moduleType"], 3);
        assert_eq!(p["operateType"], "acOutCfg");
        assert_eq!(p["params"]["enabled"], 1);
        // The params the verified command carries, at the values the device has.
        assert_eq!(
            p["params"]["xboost"], 1,
            "inv.cfgAcXboost echoed from the device"
        );
        assert_eq!(p["params"]["out_voltage"], -1, "the as-is sentinel");
        assert_eq!(p["params"]["out_freq"], 255, "the as-is sentinel");
    }

    #[test]
    fn a_command_carries_the_full_param_set_the_device_answers() {
        // Publishing only the picked param was accepted by the broker, half-applied
        // (`inv.SlowChgWatts` moved to a value never sent), and never answered. The
        // full param set the integrations send is answered with code 0, so every
        // param of the verified command travels, echoed from the device's own value.
        let raw = live();
        let watts = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_charge_watts")
            .expect("ac charge watts");
        let p =
            serde_json::from_str::<Value>(&set_payload(watts, 900, 999900002, &raw)).expect("json");
        assert_eq!(
            p["params"]["slowChgWatts"], 900,
            "the value the user picked"
        );
        assert_eq!(p["params"]["fastChgWatts"], 2400, "echoed from the device");
        assert_eq!(p["params"]["chgPauseFlag"], 0, "echoed from the device");

        let lcd = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "lcd_timeout")
            .expect("lcd timeout");
        let p =
            serde_json::from_str::<Value>(&set_payload(lcd, 300, 999900003, &raw)).expect("json");
        assert_eq!(p["params"]["delayOff"], 300);
        assert_eq!(
            p["params"]["brighLevel"], 100,
            "brightness echoed, not changed"
        );

        // A setting the device did not report travels as the as-is sentinel, never
        // as a value invented by the app.
        let mut missing = raw.clone();
        missing.remove("inv.cfgAcXboost");
        let ac = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        let p =
            serde_json::from_str::<Value>(&set_payload(ac, 1, 999900004, &missing)).expect("json");
        assert_eq!(
            p["params"]["xboost"], 255,
            "unreported settings stay as they are"
        );
    }

    #[test]
    fn every_command_in_a_session_gets_its_own_id() {
        // The device echoes the id in the reply, so a repeated id cannot be matched
        // to the command it answers.
        let raw = live();
        let c = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "beep")
            .expect("beep");
        let a = serde_json::from_str::<Value>(&set_payload(c, 1, BASE_ID, &raw)).expect("json");
        let b = serde_json::from_str::<Value>(&set_payload(c, 1, BASE_ID + 1, &raw)).expect("json");
        assert_eq!(a["id"].as_u64().unwrap(), BASE_ID);
        assert_eq!(b["id"].as_u64().unwrap(), BASE_ID + 1);
        assert_eq!(a["from"], "wattdeck");
        assert_eq!(a["version"], "1.0");
    }

    #[test]
    fn values_outside_the_accepted_range_are_refused() {
        let c = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "charge_limit")
            .expect("charge limit");
        assert!(c.accepts(100) && c.accepts(60));
        assert!(!c.accepts(101), "past the top of the range");
        assert!(
            !c.accepts(10),
            "the app does not offer a charge limit the device would reject"
        );
        let ac = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        assert!(ac.accepts(0) && ac.accepts(1));
        assert!(!ac.accepts(2), "a toggle takes 0 or 1");
    }

    #[test]
    fn the_dialog_names_the_value_in_the_units_the_device_uses() {
        let c = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_charge_watts")
            .expect("ac charge power");
        assert_eq!(
            Pending {
                control: c,
                value: 1200
            }
            .describe(),
            "AC charge power = 1200 W"
        );
        let ac = for_family(Family::Delta2)
            .iter()
            .find(|c| c.id == "ac_output")
            .expect("ac output");
        assert_eq!(
            Pending {
                control: ac,
                value: 0
            }
            .describe(),
            "AC output = off"
        );
    }

    #[test]
    fn every_control_reads_the_key_the_live_device_reports() {
        // The read key must be present in the live 240-key property dump, so the
        // dialog can show the current value. Checked against the fixture the app
        // already uses.
        // The fixture is the masked live dump: 240 keys a DELTA 2 Max reported.
        let dump: std::collections::HashMap<String, Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/quota_all.json"))
                .expect("fixture");
        for c in for_family(Family::Delta2) {
            assert!(
                dump.contains_key(c.read_key),
                "{} reads {key}, which the live device does not report",
                c.id,
                key = c.read_key
            );
        }
    }
}
