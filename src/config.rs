// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 3]
pub struct Config {
    pub sample_interval_ms: u64,
    /// User-selected liquidctl device description (verbatim, used as
    /// `--match` substring). `None` means auto-detect at runtime.
    pub device_match: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1500,
            device_match: None,
        }
    }
}
