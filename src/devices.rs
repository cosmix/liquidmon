// SPDX-License-Identifier: MPL-2.0

//! AIO device classification: substring catalog and helpers used to filter the
//! liquidctl-enumerated device list down to known-compatible AIO coolers.

use crate::liquidctl::DetectedDevice;

/// Substring catalog used to identify AIO devices among liquidctl's full
/// enumeration. Patterns are written lowercase so `is_aio` can do a single
/// `to_ascii_lowercase` on the description and skip per-call allocation
/// inside the loop. Restricted to families verified against the current
/// parser schema; see PLAN-device-selector.md "Compatibility Constraints".
const AIO_PATTERNS: &[&str] = &[
    "hydro",  // Corsair Hydro Pro / Pro XT / Platinum (hydro_platinum.py)
    "icue h", // Corsair iCUE Elite Capellix / RGB     (hydro_platinum.py)
];

/// Returns true when `description` contains any AIO_PATTERNS substring,
/// case-insensitively (matching liquidctl's own `--match` semantics).
pub fn is_aio(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    AIO_PATTERNS.iter().any(|p| d.contains(p))
}

/// Filter the enumerated device list to known AIOs only, preserving
/// liquidctl's order. The returned references borrow from the input slice.
pub fn filter_aios(devices: &[DetectedDevice]) -> Vec<&DetectedDevice> {
    devices.iter().filter(|d| is_aio(&d.description)).collect()
}

/// Pick the first AIO from a list, or `None` if no AIO is present.
pub fn auto_select(devices: &[DetectedDevice]) -> Option<&DetectedDevice> {
    devices.iter().find(|d| is_aio(&d.description))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(description: &str) -> DetectedDevice {
        DetectedDevice {
            description: description.to_string(),
            bus: "hid".to_string(),
            address: "/dev/hidraw0".to_string(),
        }
    }

    #[test]
    fn is_aio_matches_known_substrings() {
        assert!(is_aio("Corsair Hydro H150i Pro XT"));
        assert!(is_aio("Corsair iCUE H100i Elite Capellix"));
    }

    #[test]
    fn is_aio_rejects_psus_and_rgb_hubs() {
        assert!(!is_aio("Corsair RMi Series Power Supply"));
        assert!(!is_aio("Corsair Lighting Node Pro"));
        assert!(!is_aio("Gigabyte RGB Fusion 2.0 8297 Controller"));
    }

    #[test]
    fn is_aio_is_case_insensitive() {
        assert!(is_aio("corsair hydro h150i"));
        assert!(is_aio("CORSAIR ICUE H100I"));
    }

    #[test]
    fn auto_select_picks_first_aio_in_list() {
        let list = vec![
            dev("Gigabyte RGB Fusion 2.0 8297 Controller"),
            dev("Corsair Hydro H150i Pro XT"),
            dev("Corsair iCUE H100i Elite Capellix"),
        ];
        let picked = auto_select(&list).expect("should pick first AIO");
        assert_eq!(picked.description, "Corsair Hydro H150i Pro XT");
    }

    #[test]
    fn auto_select_returns_none_for_no_aio() {
        let list = vec![
            dev("Corsair RMi Series Power Supply"),
            dev("Gigabyte RGB Fusion 2.0 8297 Controller"),
        ];
        assert!(auto_select(&list).is_none());
    }

    #[test]
    fn filter_aios_preserves_order_and_drops_non_aios() {
        let list = vec![
            dev("Gigabyte RGB Fusion 2.0 8297 Controller"),
            dev("Corsair Hydro H150i Pro XT"),
            dev("Corsair RMi Series Power Supply"),
            dev("Corsair iCUE H100i Elite Capellix"),
        ];
        let aios = filter_aios(&list);
        assert_eq!(aios.len(), 2);
        assert_eq!(aios[0].description, "Corsair Hydro H150i Pro XT");
        assert_eq!(aios[1].description, "Corsair iCUE H100i Elite Capellix");
    }
}
