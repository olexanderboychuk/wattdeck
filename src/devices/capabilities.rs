/// What a device is known to report. Capabilities are set per model from verified
/// property names, never assumed. A capability that has not been observed for a
/// model stays false, so the UI never shows a control or gauge for something the
/// device cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceCapabilities {
    pub battery_monitoring: bool,
    pub battery_health: bool,
    pub battery_cells: bool,
    pub temperature_monitoring: bool,
    pub solar_input: bool,
    pub ac_input: bool,
    pub dc_input: bool,
    pub ac_output: bool,
    pub dc_output: bool,
    pub usb_output: bool,
    pub ac_output_control: bool,
    pub dc_output_control: bool,
    pub charge_limit_control: bool,
    pub extra_battery: bool,
    pub grid_connection: bool,
    pub energy_history: bool,
}

impl DeviceCapabilities {
    /// The full set used by the DELTA 2 family, verified from the live 240-key
    /// property dump of a DELTA 2 Max (productType 81).
    pub const DELTA_2_FAMILY: Self = Self {
        battery_monitoring: true,
        battery_health: true,
        battery_cells: true,
        temperature_monitoring: true,
        solar_input: true,
        ac_input: true,
        dc_input: true,
        ac_output: true,
        dc_output: true,
        usb_output: true,
        ac_output_control: true,
        dc_output_control: true,
        charge_limit_control: true,
        extra_battery: true,
        grid_connection: false,
        energy_history: true,
    };

    /// DELTA Pro family: same battery model under `bmsMaster.*` / `ems.*`.
    pub const DELTA_PRO: Self = Self {
        grid_connection: true,
        ..Self::DELTA_2_FAMILY
    };

    /// DELTA Pro 3: flat camelCase keys, verified from a public dump.
    pub const DELTA_PRO_3: Self = Self {
        battery_cells: false,
        ..Self::DELTA_PRO
    };

    /// RIVER family: smaller, single MPPT, no XT150/AC-xboost property set.
    pub const RIVER: Self = Self {
        extra_battery: false,
        battery_cells: true,
        ..Self::DELTA_2_FAMILY
    };

    /// PowerStream: solar-only, no battery.
    pub const POWERSTREAM: Self = Self {
        battery_monitoring: false,
        battery_health: false,
        battery_cells: false,
        temperature_monitoring: true,
        solar_input: true,
        ac_input: false,
        dc_input: true,
        ac_output: true,
        dc_output: true,
        usb_output: false,
        ac_output_control: false,
        dc_output_control: false,
        charge_limit_control: false,
        extra_battery: false,
        grid_connection: false,
        energy_history: true,
    };

    /// Smart Plug: metered outlet only.
    pub const SMART_PLUG: Self = Self {
        battery_monitoring: false,
        temperature_monitoring: false,
        ac_input: true,
        ac_output: true,
        energy_history: true,
        ..Self::generic()
    };

    /// Nothing is assumed for an unrecognised product type.
    pub const fn generic() -> Self {
        Self {
            battery_monitoring: false,
            battery_health: false,
            battery_cells: false,
            temperature_monitoring: false,
            solar_input: false,
            ac_input: false,
            dc_input: false,
            ac_output: false,
            dc_output: false,
            usb_output: false,
            ac_output_control: false,
            dc_output_control: false,
            charge_limit_control: false,
            extra_battery: false,
            grid_connection: false,
            energy_history: false,
        }
    }

    /// The capabilities in plain words, for the device screen. Only the verified
    /// ones are named; an empty list says nothing has been confirmed for this model.
    pub fn summary(&self) -> Vec<&'static str> {
        let flags: [(&str, bool); 16] = [
            ("battery", self.battery_monitoring),
            ("health", self.battery_health),
            ("cells", self.battery_cells),
            ("temperature", self.temperature_monitoring),
            ("solar in", self.solar_input),
            ("AC in", self.ac_input),
            ("DC in", self.dc_input),
            ("AC out", self.ac_output),
            ("DC out", self.dc_output),
            ("USB out", self.usb_output),
            ("AC control", self.ac_output_control),
            ("DC control", self.dc_output_control),
            ("charge limit", self.charge_limit_control),
            ("extra battery", self.extra_battery),
            ("grid", self.grid_connection),
            ("history", self.energy_history),
        ];
        flags
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_assumes_nothing() {
        let caps = DeviceCapabilities::generic();
        assert!(!caps.battery_monitoring);
        assert!(!caps.ac_output_control);
    }

    #[test]
    fn delta2_has_cells_and_solar() {
        let caps = DeviceCapabilities::DELTA_2_FAMILY;
        assert!(caps.battery_cells && caps.solar_input && caps.usb_output);
        assert!(!caps.grid_connection);
    }

    #[test]
    fn powerstream_has_no_battery() {
        // Looked up through the registry, so the assertion checks the table the app
        // actually uses, not just the constant.
        let caps = crate::devices::registry::lookup(76).caps;
        assert!(!caps.battery_monitoring, "no cell pack to report");
        assert!(caps.solar_input);
    }
}
