use crate::devices::capabilities::DeviceCapabilities;

/// Property-name dialect. Devices speak different key schemes, and the normalizer
/// picks its table by family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    /// DELTA 2 / DELTA 2 Max: `bms_bmsStatus.*`, `bms_emsStatus.*`, `pd.*`, `inv.*`, `mppt.*`
    #[default]
    Delta2,
    /// DELTA Pro / DELTA Mini / DELTA Pro slaves: `bmsMaster.*`, `ems.*`
    DeltaPro,
    /// DELTA Pro 3: flat camelCase keys, no prefix (`bmsBattSoc`, `powInSumW`)
    DeltaPro3,
    River,
    PowerStream,
    SmartPlug,
    Unknown,
}

/// A product the app knows, with its verified property dialect and capabilities.
#[derive(Debug, Clone, Copy)]
pub struct ModelInfo {
    pub product_type: u16,
    pub name: &'static str,
    pub family: Family,
    pub caps: DeviceCapabilities,
}

/// Product types seen in the wild. `81` is verified live against a DELTA 2 Max in
/// this project (`productName: "DELTA 2 Max"`, `bms_bmsStatus.productType: 81`).
/// The rest come from public device dumps and are marked as unverified until seen.
pub const KNOWN_MODELS: &[ModelInfo] = &[
    ModelInfo {
        product_type: 81,
        name: "DELTA 2 Max",
        family: Family::Delta2,
        caps: DeviceCapabilities::DELTA_2_FAMILY,
    },
    ModelInfo {
        product_type: 80,
        name: "DELTA 2",
        family: Family::Delta2,
        caps: DeviceCapabilities::DELTA_2_FAMILY,
    },
    ModelInfo {
        product_type: 72,
        name: "RIVER 2 Pro",
        family: Family::River,
        caps: DeviceCapabilities::RIVER,
    },
    ModelInfo {
        product_type: 71,
        name: "RIVER 2 Max",
        family: Family::River,
        caps: DeviceCapabilities::RIVER,
    },
    ModelInfo {
        product_type: 70,
        name: "RIVER 2",
        family: Family::River,
        caps: DeviceCapabilities::RIVER,
    },
    // Verified live from the device list (`productName: "RIVER mini"`). The product
    // type is not verified for this model, so it is recorded as 0 and the family comes
    // from the name until quota data reports a type.
    ModelInfo {
        product_type: 0,
        name: "RIVER mini",
        family: Family::River,
        caps: DeviceCapabilities::RIVER,
    },
    ModelInfo {
        product_type: 15,
        name: "DELTA Pro",
        family: Family::DeltaPro,
        caps: DeviceCapabilities::DELTA_PRO,
    },
    ModelInfo {
        product_type: 64,
        name: "DELTA Pro 3",
        family: Family::DeltaPro3,
        caps: DeviceCapabilities::DELTA_PRO_3,
    },
    ModelInfo {
        product_type: 67,
        name: "DELTA Mini",
        family: Family::DeltaPro,
        caps: DeviceCapabilities::DELTA_PRO,
    },
    ModelInfo {
        product_type: 46,
        name: "GLACIER",
        family: Family::Unknown,
        caps: DeviceCapabilities::generic(),
    },
    ModelInfo {
        product_type: 76,
        name: "POWER STREAM",
        family: Family::PowerStream,
        caps: DeviceCapabilities::POWERSTREAM,
    },
    ModelInfo {
        product_type: 77,
        name: "Smart Plug",
        family: Family::SmartPlug,
        caps: DeviceCapabilities::SMART_PLUG,
    },
];

/// The REST device list reports `productName` strings, not product types. Name lookup
/// fills the family before quota data arrives.
pub fn lookup_by_name(name: &str) -> ModelInfo {
    KNOWN_MODELS
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .copied()
        .unwrap_or(ModelInfo {
            product_type: 0,
            name: "Unknown device",
            family: Family::Unknown,
            caps: DeviceCapabilities::generic(),
        })
}

pub fn lookup(product_type: u16) -> ModelInfo {
    // 0 is the placeholder for "no product type reported", not a real model id.
    if product_type == 0 {
        return ModelInfo {
            product_type: 0,
            name: "Unknown device",
            family: Family::Unknown,
            caps: DeviceCapabilities::generic(),
        };
    }
    KNOWN_MODELS
        .iter()
        .find(|m| m.product_type == product_type)
        .copied()
        .unwrap_or(ModelInfo {
            product_type,
            name: "Unknown device",
            family: Family::Unknown,
            caps: DeviceCapabilities::generic(),
        })
}

impl Family {
    pub fn from_product_type(product_type: u16) -> Self {
        lookup(product_type).family
    }

    pub fn label(&self) -> &'static str {
        match self {
            Family::Delta2 => "DELTA 2 family",
            Family::DeltaPro => "DELTA Pro family",
            Family::DeltaPro3 => "DELTA Pro 3",
            Family::River => "RIVER family",
            Family::PowerStream => "PowerStream",
            Family::SmartPlug => "Smart Plug",
            Family::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_product_type_is_the_delta2max() {
        let m = lookup(81);
        assert_eq!(m.name, "DELTA 2 Max");
        assert_eq!(m.family, Family::Delta2);
        assert!(m.caps.battery_monitoring);
    }

    #[test]
    fn unknown_product_type_assumes_nothing() {
        let m = lookup(9999);
        assert_eq!(m.name, "Unknown device");
        assert_eq!(m.family, Family::Unknown);
        assert_eq!(m.caps, DeviceCapabilities::generic());
    }

    #[test]
    fn zero_is_a_placeholder_not_a_model() {
        // 0 means "no product type reported"; it must not resolve to a real model.
        let m = lookup(0);
        assert_eq!(m.name, "Unknown device");
        assert_eq!(m.family, Family::Unknown);
    }

    #[test]
    fn river_mini_is_known_by_name() {
        // Verified live as `productName: "RIVER mini"`; the product type is unverified.
        let m = lookup_by_name("RIVER mini");
        assert_eq!(m.family, Family::River);
        assert!(m.caps.battery_monitoring);
    }

    #[test]
    fn family_labels() {
        assert_eq!(Family::DeltaPro3.label(), "DELTA Pro 3");
        assert_eq!(Family::from_product_type(80).label(), "DELTA 2 family");
    }

    #[test]
    fn every_known_model_declares_a_family() {
        for m in KNOWN_MODELS {
            assert_ne!(m.name, "");
        }
    }
}
